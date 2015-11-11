extern crate clap;
extern crate hyper;
extern crate iron;
extern crate regex;
extern crate router;
extern crate serde_json;
extern crate toml;

use clap::App;
use iron::*;
use toml::Table;

use std::collections::LinkedList;
use std::fs::File;
use std::io::Read;
use std::process::{Command, ExitStatus};
use regex::Regex;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

#[derive(Debug)]
struct BuildRequest {
    checkout_sha: String,
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
    let checkout_sha = obj.get("checkout_sha").unwrap();
    let arg = checkout_sha.as_string().unwrap();

    let incoming = BuildRequest {
        checkout_sha: arg.to_string(),
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
    loop {
        let arg;
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

        let mut child = Command::new("wget")
            .arg("-S").arg("-O-")
            .arg("--no-check-certificate").arg("--auth-no-challenge")
            .arg(format!("--http-user={}", http_user))
            .arg(format!("--http-password={}", http_password))
            .arg(format!("{}/?token={}&cause=I+want+to+be+built", jenkins_job_url, token))
            .current_dir("workspace/shurik")
            .spawn()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        let output = child.wait_with_output().unwrap();
        let status = output.status;
        let stderr = output.stderr;
        let stderr = std::str::from_utf8(&stderr).unwrap();
        if ! ExitStatus::success(&status) {
            panic!("Couldn't notify the Jenkins: {}", status)
        }

        println!("Notified the Jenkins");

        let re = Regex::new(r"Location: ([^\n]*)").unwrap();
        let location = re.captures(stderr).unwrap().at(1).unwrap();
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
