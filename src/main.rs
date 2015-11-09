extern crate clap;
extern crate iron;
extern crate router;
extern crate serde_json;

use clap::App;
use iron::*;

use std::process::{Command, ExitStatus};
use std::io::Read;

#[derive(Debug)]
struct MyError;

impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "")
    }
}

impl std::error::Error for MyError {
    fn description(&self) -> &str {
        ""
    }
}

fn handle_mr(req: &mut Request) -> IronResult<Response> {
    let ref mut body = req.body;
    let mut s: String = String::new();
    body.read_to_string(&mut s).unwrap();

    println!("{}", req.url);
    println!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    println!("data: {:?}", json);
    // data: {"bar":"baz","foo":13}
    println!("object? {}", json.is_object());
    let obj = json.as_object().unwrap();
    let checkout_sha = obj.get("checkout_sha").unwrap();
    let arg = checkout_sha.as_string().unwrap();

    let status = Command::new("git")
        .arg("checkout").arg(arg)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ! ExitStatus::success(&status) {
        panic!("Couldn't checkout the workspace: {}", status)
    }

    let status = Command::new("cargo")
        .arg("build")
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ! ExitStatus::success(&status) {
        panic!("Couldn't build in the workspace: {}", status)
    }

    return Ok(Response::with(status::Ok));
    Err(iron::error::IronError::new(MyError, status::BadRequest))
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

    println!("Dry run: {}", matches.is_present("dry-run"));
    let gitlab_port =
        matches.value_of("GITLAB_PORT").unwrap_or("10042").parse().unwrap_or(10042);
    let gitlab_address =
        matches.value_of("GITLAB_ADDRESS").unwrap_or("localhost");
    println!("GitLab port: {}", gitlab_port);
    println!("GitLab address: {}", gitlab_address);

    let mut router = router::Router::new();
    router.post("/api/v1/mr",
                move |req: &mut Request|
                handle_mr(req));
    Iron::new(router).http(
        (gitlab_address, gitlab_port))
        .expect("Couldn't start the web server");
}
