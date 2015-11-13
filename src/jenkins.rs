use ::std::process::{Command, ExitStatus};
use ::regex::Regex;
use ::serde_json;
use std::time::Duration;
use ::std;
use ::iron::*;

pub fn enqueue_build(user: &str, password: &str, job_url: &str, token: &str) -> String {
    let output = Command::new("wget")
        .arg("-S").arg("-O-")
        .arg("--no-check-certificate").arg("--auth-no-challenge")
        .arg(format!("--http-user={}", user))
        .arg(format!("--http-password={}", password))
        .arg(format!("{}/?token={}&cause=I+want+to+be+built", job_url, token))
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
    location.to_owned()
}

pub fn poll_queue(user: &str, password: &str, location: &str) -> String {
    let arg;
    loop {
        let output = Command::new("wget")
            .arg("-S").arg("-O-")
            .arg("--no-check-certificate").arg("--auth-no-challenge")
            .arg(format!("--http-user={}", user))
            .arg(format!("--http-password={}", password))
            .arg(format!("{}/api/json?pretty=true", location))
            .current_dir("workspace/shurik")
            .output()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        let status = output.status;
        let stdout = output.stdout;
        let stdout = ::std::str::from_utf8(&stdout).unwrap();
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
        ::std::thread::sleep(Duration::new(5, 0));
        println!("ok");
    }
    arg
}

pub fn poll_build(user: &str, password: &str, build_url: &str) -> String {
    let result_string;
    loop {
        let output = Command::new("wget")
            .arg("-S").arg("-O-")
            .arg("--no-check-certificate").arg("--auth-no-challenge")
            .arg(format!("--http-user={}", user))
            .arg(format!("--http-password={}", password))
            .arg(format!("{}/api/json?pretty=true", build_url))
            .current_dir("workspace/shurik")
            .output()
            .unwrap_or_else(|e| {
                panic!("failed to execute process: {}", e)
            });
        let status = output.status;
        let stdout = output.stdout;
        let stdout = std::str::from_utf8(&stdout).unwrap();
        if ! ExitStatus::success(&status) {
            panic!("Couldn't poll the Jenkins build: {}", status)
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

                break;
            }
        }

        print!("Sleeping...");
        std::thread::sleep(Duration::new(5, 0));
        println!("ok");
    }
    result_string
}