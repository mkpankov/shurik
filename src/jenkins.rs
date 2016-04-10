use ::std::process::{Command, ExitStatus};
use ::regex::Regex;
use ::serde_json;
use std::time::Duration;
use ::std;
use ::iron::*;

pub fn enqueue_build(workspace_dir: &str, user: &str, password: &str, job_url: &str, run_type: &str) -> String {
    let output = Command::new("wget")
        .arg("-S").arg("-O-")
        .arg("--no-check-certificate").arg("--auth-no-challenge")
        .arg("--method=POST")
        .arg(format!("--http-user={}", user))
        .arg(format!("--http-password={}", password))
        .arg(format!("{}/?cause=I+want+to+be+built&RUN_TYPE={}", job_url, run_type))
        .current_dir(workspace_dir)
        .output()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    let status = output.status;
    let stdout = output.stdout;
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = output.stderr;
    let stderr = String::from_utf8_lossy(&stderr);
    if ! ExitStatus::success(&status) {
        panic!("Couldn't notify the Jenkins: {}. Standard output: {}. Standard error: {}", status, stdout, stderr);
    }

    info!("Notified the Jenkins");

    let re = Regex::new(r"Location: ([^\n]*)").unwrap();
    let location = re.captures(&stderr).unwrap().at(1).unwrap();
    location.to_owned()
}

pub fn poll_queue(workspace_dir: &str, user: &str, password: &str, location: &str) -> String {
    let arg;
    loop {
        let output = Command::new("wget")
            .arg("-S").arg("-O-")
            .arg("--no-check-certificate").arg("--auth-no-challenge")
            .arg(format!("--http-user={}", user))
            .arg(format!("--http-password={}", password))
            .arg(format!("{}/api/json?pretty=true", location))
            .current_dir(workspace_dir)
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

        info!("Got queue item");

        let json: serde_json::value::Value = serde_json::from_str(&stdout).unwrap();
        debug!("data: {:?}", json);
        debug!("object? {}", json.is_object());
        let obj = json.as_object().unwrap();
        if let Some(executable) = obj.get("executable") {
            let url = executable.as_object().unwrap().get("url").unwrap();
            arg = url.as_string().unwrap().to_string();
            debug!("Parsed the final url");
            break;
        }

        print!("Sleeping...");
        ::std::thread::sleep(Duration::new(5, 0));
        println!("ok");
    }
    arg
}

pub fn poll_build(workspace_dir: &str, user: &str, password: &str, build_url: &str) -> String {
    let result_string;
    loop {
        let output = Command::new("wget")
            .arg("-S").arg("-O-")
            .arg("--no-check-certificate").arg("--auth-no-challenge")
            .arg(format!("--http-user={}", user))
            .arg(format!("--http-password={}", password))
            .arg(format!("{}/api/json?pretty=true", build_url))
            .current_dir(workspace_dir)
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

        info!("Polled");

        let json: serde_json::value::Value = serde_json::from_str(&stdout).unwrap();
        debug!("data: {:?}", json);
        debug!("object? {}", json.is_object());
        let obj = json.as_object().unwrap();
        if let Some(result) = obj.get("result") {
            if ! result.is_null() {
                result_string = result.as_string().unwrap().to_owned();
                debug!("Parsed response, result_string == {}", result_string);

                break;
            }
        }

        print!("Sleeping...");
        std::thread::sleep(Duration::new(5, 0));
        println!("ok");
    }
    result_string
}
