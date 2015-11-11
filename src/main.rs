extern crate clap;
extern crate hyper;
extern crate iron;
extern crate regex;
extern crate router;
extern crate serde_json;
extern crate toml;

use clap::App;
use hyper::Client;
use hyper::header::{ContentType};
use hyper::mime::{Mime, TopLevel, SubLevel};
use iron::*;
use toml::Table;

use std::collections::LinkedList;
use std::fs::File;
use std::io::Read;
use std::process::{Command, ExitStatus};
use regex::Regex;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct BuildRequest {
    checkout_sha: String,
    source_project_id: String,
    mr_id: String
}

fn handle_mr(req: &mut Request, queue: &(Mutex<LinkedList<BuildRequest>>, Condvar))
             -> IronResult<Response> {
    let &(ref list, ref cvar) = queue;
    let ref mut body = req.body;
    let mut s: String = String::new();
    body.read_to_string(&mut s).unwrap();

    println!("{}", req.url);
    println!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    println!("data: {:?}", json);
    println!("object? {}", json.is_object());
    let obj = json.as_object().unwrap();
    let checkout_sha = obj.get("checkout_sha").unwrap().as_string().unwrap();
    let source_project_id = obj.get("source_project_id").unwrap().as_string().unwrap();
    let mr_id = obj.get("id").unwrap().as_string().unwrap();

    let incoming = BuildRequest {
        checkout_sha: checkout_sha.to_owned(),
        source_project_id: source_project_id.to_owned(),
        mr_id: mr_id.to_owned(),
    };
    let mut queue = list.lock().unwrap();
    queue.push_back(incoming);
    println!("Queued up...");
    cvar.notify_one();
    println!("Notified...");

    return Ok(Response::with(status::Ok));
}

fn handle_build_request(queue: &(Mutex<LinkedList<BuildRequest>>, Condvar), config: Table) -> !
{
    let gitlab_user = config.get("user").unwrap().as_str().unwrap();
    let gitlab_password = config.get("password").unwrap().as_str().unwrap();
    let gitlab_api_root = config.get("url").unwrap().as_str().unwrap();

    let client = Client::new();

    let mut res = client.post(&*format!("{}/session", gitlab_api_root))
        .body(&*format!("login={}&password={}", gitlab_user, gitlab_password))
        .send()
        .unwrap();
    assert_eq!(res.status, hyper::status::StatusCode::Created);
    let mut text = String::new();
    res.read_to_string(&mut text).unwrap();
    let json: serde_json::value::Value = serde_json::from_str(&text).unwrap();
    println!("data: {:?}", json);
    println!("object? {}", json.is_object());
    let obj = json.as_object().unwrap();
    let private_token = obj.get("private_token").unwrap().as_string().unwrap();
    println!("Logged in to GitLab, token == {}", private_token);

    loop {
        let arg;
        let source_project_id;
        let mr_id;

        {
            let &(ref list, ref cvar) = queue;
            println!("Waiting to get the request...");
            let mut list = list.lock().unwrap();
            while list.is_empty() {
                list = cvar.wait(list).unwrap();
            }
            let request = list.pop_front().unwrap();
            println!("Got the request");
            arg = request.checkout_sha;
            source_project_id = request.source_project_id;
            mr_id = request.mr_id;
        }

        let status = Command::new("git")
            .arg("fetch")
            .current_dir("workspace/shurik")
            .status()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        if ! ExitStatus::success(&status) {
            panic!("Couldn't fetch remote: {}", status)
        }
        println!("Fetched remote");

        let status = Command::new("git")
            .arg("checkout").arg("try")
            .current_dir("workspace/shurik")
            .status()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        if ! ExitStatus::success(&status) {
            panic!("Couldn't checkout the 'try' branch: {}", status)
        }
        println!("Checked out 'try'");

        let status = Command::new("git")
            .arg("reset").arg("--hard").arg(&arg)
            .current_dir("workspace/shurik")
            .status()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        if ! ExitStatus::success(&status) {
            panic!("Couldn't reset the 'try' branch: {}", status)
        }
        println!("Reset 'try' to {}", arg);

        let status = Command::new("git")
            .arg("push").arg("--force-with-lease")
            .current_dir("workspace/shurik")
            .status()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        if ! ExitStatus::success(&status) {
            panic!("Couldn't push the 'try' branch: {}", status)
        }
        println!("Push 'try'");

        let http_user = config.get("http-user").unwrap().as_str().unwrap();
        let http_password = config.get("http-password").unwrap().as_str().unwrap();
        let token = config.get("token").unwrap().as_str().unwrap();
        let jenkins_job_url = config.get("jenkins-job-url").unwrap().as_str().unwrap();
        println!("{} {} {} {}", http_user, http_password, token, jenkins_job_url);

        let output = Command::new("wget")
            .arg("-S").arg("-O-")
            .arg("--no-check-certificate").arg("--auth-no-challenge")
            .arg(format!("--http-user={}", http_user))
            .arg(format!("--http-password={}", http_password))
            .arg(format!("{}/?token={}&cause=I+want+to+be+built", jenkins_job_url, token))
            .current_dir("workspace/shurik")
            .output()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        let status = output.status;
        let stderr = output.stderr;
        let stderr = String::from_utf8_lossy(&stderr);
        if ! ExitStatus::success(&status) {
            panic!("Couldn't notify the Jenkins: {}", status)
        }

        println!("Notified the Jenkins");

        let re = Regex::new(r"Location: ([^\n]*)").unwrap();
        let location = re.captures(&stderr).unwrap().at(1).unwrap();

        println!("Parsed the location == {}", location);

        let arg;
        loop {
            let output = Command::new("wget")
                .arg("-S").arg("-O-")
                .arg("--no-check-certificate").arg("--auth-no-challenge")
                .arg(format!("--http-user={}", http_user))
                .arg(format!("--http-password={}", http_password))
                .arg(format!("{}/api/json?pretty=true", location))
                .current_dir("workspace/shurik")
                .output()
                .unwrap_or_else(|e| {
                    panic!("failed to execute process: {}", e)
                });
            let status = output.status;
            let stdout = output.stdout;
            let stdout = std::str::from_utf8(&stdout).unwrap();
            if ! ExitStatus::success(&status) {
                panic!("Couldn't get queue item from Jenkins: {}", status)
            }

            println!("Got queue item");

            let json: serde_json::value::Value = serde_json::from_str(&stdout).unwrap();
            println!("data: {:?}", json);
            println!("object? {}", json.is_object());
            let obj = json.as_object().unwrap();
            if let Some(executable) = obj.get("executable") {
                let url = executable.as_object().unwrap().get("url").unwrap();
                arg = url.as_string().unwrap().to_string();
                println!("Parsed the final url");
                break;
            }

            print!("Sleeping...");
            std::thread::sleep(Duration::new(5, 0));
            println!("ok");
        }

        loop {
            let output = Command::new("wget")
                .arg("-S").arg("-O-")
                .arg("--no-check-certificate").arg("--auth-no-challenge")
                .arg(format!("--http-user={}", http_user))
                .arg(format!("--http-password={}", http_password))
                .arg(format!("{}/api/json?pretty=true", arg))
                .current_dir("workspace/shurik")
                .output()
                .unwrap_or_else(|e| {
                    panic!("failed to execute process: {}", e)
                });
            let status = output.status;
            let stdout = output.stdout;
            let stdout = std::str::from_utf8(&stdout).unwrap();
            if ! ExitStatus::success(&status) {
                panic!("Couldn't notify the Jenkins: {}", status)
            }

            println!("Polled");

            let json: serde_json::value::Value = serde_json::from_str(&stdout).unwrap();
            println!("data: {:?}", json);
            println!("object? {}", json.is_object());
            let obj = json.as_object().unwrap();
            if let Some(result) = obj.get("result") {
                if ! result.is_null() {
                    let result = result.as_string().unwrap();
                    println!("Parsed response, result == {}", result);

                    let mut headers = Headers::new();
                    headers.set(ContentType(Mime(TopLevel::Application, SubLevel::Json, vec![])));
                    let res = client.post(&*format!("{}/projects/{}/merge_request/{}/comments", gitlab_api_root, source_project_id, mr_id))
                        .headers(headers)
                        .body(&*format!("login={}&password={}", gitlab_user, gitlab_password))
                        .send()
                        .unwrap();
                    assert_eq!(res.status, hyper::status::StatusCode::Created);
                }
            }

            print!("Sleeping...");
            std::thread::sleep(Duration::new(5, 0));
            println!("ok");
        }
    }
}

fn main() {
    let matches =
        App::new("shurik")
        .version("0.1")
        .author("Mikhail Pankov <mikhail.pankov@kaspersky.com>")
        .about("A commit gatekeeper for SDK")
        .args_from_usage(
            "-n --dry-run 'Don\'t actually do anything, just print what is to be done'
             -p --gitlab-port=[GITLAB_PORT] 'Port to listen for GitLab WebHooks'
             -a --gitlab-address=[GITLAB_ADDRESS] 'Address to listen for GitLab WebHooks'")
        .get_matches();

    let mut file = File::open("Config.toml").unwrap();
    let mut toml = String::new();
    file.read_to_string(&mut toml).unwrap();

    let mut parser = toml::Parser::new(&toml);
    let config = parser.parse().unwrap();

    println!("Dry run: {}", matches.is_present("dry-run"));
    let gitlab_port =
        matches.value_of("GITLAB_PORT").unwrap_or("10042").parse().unwrap_or(10042);
    let gitlab_address =
        matches.value_of("GITLAB_ADDRESS").unwrap_or("localhost");
    println!("GitLab port: {}", gitlab_port);
    println!("GitLab address: {}", gitlab_address);

    let queue = Arc::new((Mutex::new(LinkedList::new()), Condvar::new()));
    let queue2 = queue.clone();

    let builder = thread::spawn(move || {
        handle_build_request(&*queue, config);
    });

    let mut router = router::Router::new();
    router.post("/api/v1/mr",
                move |req: &mut Request|
                handle_mr(req, &*queue2));
    Iron::new(router).http(
        (gitlab_address, gitlab_port))
        .expect("Couldn't start the web server");
    builder.join().unwrap();
}
