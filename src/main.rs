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

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum SubStatusOpen {
    WaitingForReview,
    WaitingForCi,
    WaitingForMerge,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Status {
    Open(SubStatusOpen),
    Merged,
    Closed,
}

#[derive(Debug)]
struct BuildRequest {
    checkout_sha: String,
    target_project_id: u64,
    mr_id: u64,
    status: Status,
    mr_human_number: u64,
}

#[derive(Debug)]
struct MrUid {
    target_project_id: u64,
    mr_id: u64,
}

#[allow(unused)]
fn find_mr(list: &LinkedList<BuildRequest>, id: MrUid) -> Option<&BuildRequest> {
    for i in list.iter() {
        if i.target_project_id == id.target_project_id
            && i.mr_id == id.mr_id
        {
            return Some(i);
        }
    }
    None
}

fn find_mr_mut(list: &mut LinkedList<BuildRequest>, id: MrUid) -> Option<&mut BuildRequest> {
    for i in list.iter_mut() {
        if i.target_project_id == id.target_project_id
            && i.mr_id == id.mr_id
        {
            return Some(i);
        }
    }
    None
}

fn handle_mr(req: &mut Request, queue: &(Mutex<LinkedList<BuildRequest>>, Condvar))
             -> IronResult<Response> {
    let &(ref list, _) = queue;
    let ref mut body = req.body;
    let mut s: String = String::new();
    body.read_to_string(&mut s).unwrap();

    println!("{}", req.url);
    println!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    println!("object? {}", json.is_object());
    let obj = json.as_object().unwrap();
    let attrs = obj.get("object_attributes").unwrap().as_object().unwrap();
    let last_commit = attrs.get("last_commit").unwrap().as_object().unwrap();
    let checkout_sha = last_commit.get("id").unwrap().as_string().unwrap();
    let target_project_id = attrs.get("target_project_id").unwrap().as_u64().unwrap();
    let mr_human_number = attrs.get("iid").unwrap().as_u64().unwrap();
    let mr_id = attrs.get("id").unwrap().as_u64().unwrap();
    let action = json.lookup("object_attributes.action").unwrap().as_string().unwrap();
    let new_status = match action {
        "open" | "reopen" => Status::Open(SubStatusOpen::WaitingForReview),
        "close" => Status::Closed,
        "merge" => Status::Merged,
        _ => panic!("Unexpected MR action: {}", action),
    };

    if let Some(mut existing_mr) =
        find_mr_mut(
            &mut *list.lock().unwrap(),
            MrUid { target_project_id: target_project_id, mr_id: mr_id })
    {
        existing_mr.status = new_status;
        println!("Updated existing MR");
    } else {
        let incoming = BuildRequest {
            checkout_sha: checkout_sha.to_owned(),
            target_project_id: target_project_id,
            mr_id: mr_id.to_owned(),
            status: new_status,
            mr_human_number: mr_human_number,
        };
        let mut queue = list.lock().unwrap();
        queue.push_back(incoming);
        println!("Queued up...");
    }

    return Ok(Response::with(status::Ok));
}

fn handle_comment(req: &mut Request, queue: &(Mutex<LinkedList<BuildRequest>>, Condvar))
                  -> IronResult<Response> {
    let &(ref list, ref cvar) = queue;
    let ref mut body = req.body;
    let mut s: String = String::new();
    body.read_to_string(&mut s).unwrap();

    println!("{}", req.url);
    println!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    println!("object? {}", json.is_object());
    let obj = json.as_object().unwrap();
    let user = obj.get("user").unwrap().as_object().unwrap();
    let username = user.get("username").unwrap().as_string().unwrap();

    if username == "pankov" {
        let attrs = obj.get("object_attributes").unwrap().as_object().unwrap();
        let note = attrs.get("note").unwrap().as_string().unwrap();
        let project_id = attrs.get("project_id").unwrap().as_u64().unwrap();
        let mr_id = attrs.get("noteable_id").unwrap().as_u64().unwrap();
        if note == "@shurik r+" {
            let mut list = list.lock().unwrap();
            for i in list.iter_mut() {
                if i.target_project_id == project_id
                    && i.mr_id == mr_id
                {
                    println!("Found");
                    i.status = Status::Open(SubStatusOpen::WaitingForCi);
                    println!("Updated");
                    break;
                }
            }
        }
    }

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
    println!("object? {}", json.is_object());
    let obj = json.as_object().unwrap();
    let private_token = obj.get("private_token").unwrap().as_string().unwrap();
    println!("Logged in to GitLab, token == {}", private_token);

    loop {
        let arg;
        let target_project_id;
        let mr_id;
        let mr_human_number;

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
            target_project_id = request.target_project_id;
            mr_id = request.mr_id;
            mr_human_number = request.mr_human_number;
            println!("{:?}", request.status);
            assert_eq!(request.status, Status::Open(SubStatusOpen::WaitingForCi));
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

        let mut result_string = String::new();
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
                    result_string = result.as_string().unwrap().to_owned();
                    println!("Parsed response, result_string == {}", result_string);

                    let mut headers = Headers::new();
                    headers.set(ContentType(Mime(TopLevel::Application, SubLevel::Json, vec![])));
                    headers.set_raw("PRIVATE-TOKEN", vec![private_token.to_owned().into_bytes()]);

                    println!("headers == {:?}", headers);

                    let message = &*format!("{{ \"note\": \"build status: {}, url: {}\"}}", result_string, arg);

                    let res = client.post(&*format!("{}/projects/{}/merge_request/{}/comments", gitlab_api_root, target_project_id, mr_id))
                        .headers(headers)
                        .body(message)
                        .send()
                        .unwrap();
                    assert_eq!(res.status, hyper::status::StatusCode::Created);
                    break;
                }
            }

            print!("Sleeping...");
            std::thread::sleep(Duration::new(5, 0));
            println!("ok");
        }

        {
            println!("{}", result_string);
            if result_string == "success" {
                ;
            }
            let &(ref list, _) = queue;
            let mut list = list.lock().unwrap();
            for i in list.iter_mut() {
                if i.target_project_id == target_project_id
                    && i.mr_id == mr_id
                {
                    println!("Found");
                    assert_eq!(i.status, Status::Open(SubStatusOpen::WaitingForCi));
                    i.status = Status::Open(SubStatusOpen::WaitingForMerge);
                    println!("Updated");
                    break;
                }
            }
        }

        {
            let status = Command::new("git")
                .arg("checkout").arg("master")
                .current_dir("workspace/shurik")
                .status()
                .unwrap_or_else(|e| {
                    panic!("failed to execute process: {}", e)
                });
            if ! ExitStatus::success(&status) {
                panic!("Couldn't checkout the 'master' branch: {}", status)
            }
            println!("Checked out 'master'");

            let status = Command::new("git")
                .arg("merge").arg("try").arg("--ff-only")
                .arg(&*format!("-m \"Merging MR #{}\"", mr_human_number))
                .current_dir("workspace/shurik")
                .status()
                .unwrap_or_else(|e| {
                    panic!("failed to execute process: {}", e)
                });
            if ! ExitStatus::success(&status) {
                panic!("Couldn't merge the 'master' branch: {}", status)
            }
            println!("Merge master to MR {}", mr_human_number);

            let status = Command::new("git")
                .arg("push")
                .current_dir("workspace/shurik")
                .status()
                .unwrap_or_else(|e| {
                    panic!("failed to execute process: {}", e)
                });
            if ! ExitStatus::success(&status) {
                panic!("Couldn't push the 'master' branch: {}", status)
            }
            println!("Push 'master'");
        }

        {
            println!("{}", result_string);
            if result_string == "success" {
                ;
            }
            let &(ref list, _) = queue;
            let mut list = list.lock().unwrap();
            for i in list.iter_mut() {
                if i.target_project_id == target_project_id
                    && i.mr_id == mr_id
                {
                    println!("Found");
                    assert_eq!(i.status, Status::Open(SubStatusOpen::WaitingForMerge));
                    i.status = Status::Merged;
                    println!("Updated");
                    break;
                }
            }
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
    let queue3 = queue.clone();

    let builder = thread::spawn(move || {
        handle_build_request(&*queue, config);
    });

    let mut router = router::Router::new();
    router.post("/api/v1/mr",
                move |req: &mut Request|
                handle_mr(req, &*queue2));
    router.post("/api/v1/comment",
                move |req: &mut Request|
                handle_comment(req, &*queue3));
    Iron::new(router).http(
        (gitlab_address, gitlab_port))
        .expect("Couldn't start the web server");
    builder.join().unwrap();
}
