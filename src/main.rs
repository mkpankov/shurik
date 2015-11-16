extern crate clap;
extern crate hyper;
extern crate iron;
extern crate regex;
extern crate router;
extern crate serde_json;
extern crate time;
extern crate toml;

use clap::App;
use hyper::Client;
use iron::*;

use std::collections::LinkedList;
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

mod git;
mod jenkins;
mod gitlab;

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

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum MergeStatus {
    CanBeMerged,
    CanNotBeMerged,
}

#[derive(Debug)]
struct MergeRequest {
    id: MrUid,
    human_number: u64,
    ssh_url: String,
    checkout_sha: String,
    status: Status,
    approval_status: ApprovalStatus,
    merge_status: MergeStatus,
}

#[derive(Debug, Clone, Copy)]
pub struct MrUid {
    target_project_id: u64,
    id: u64,
}

#[allow(unused)]
fn find_mr(list: &LinkedList<MergeRequest>, id: MrUid) -> Option<&MergeRequest> {
    for i in list.iter() {
        if i.id.target_project_id == id.target_project_id
            && i.id.id == id.id
        {
            println!("{:?}", i);
            return Some(i);
        }
    }
    None
}

fn find_mr_mut(list: &mut LinkedList<MergeRequest>, id: MrUid) -> Option<&mut MergeRequest> {
    for i in list.iter_mut() {
        if i.id.target_project_id == id.target_project_id
            && i.id.id == id.id
        {
            println!("{:?}", i);
            return Some(i);
        }
    }
    None
}

struct MergeRequestBuilder {
    id: MrUid,
    ssh_url: String,
    human_number: u64,
    checkout_sha: Option<String>,
    status: Option<Status>,
    approval_status: Option<ApprovalStatus>,
    merge_status: Option<MergeStatus>,
}

impl MergeRequestBuilder {
    fn new(id: MrUid, ssh_url: String, human_number: u64) -> Self {
        MergeRequestBuilder {
            id: id,
            ssh_url: ssh_url,
            human_number: human_number,
            checkout_sha: None,
            status: None,
            approval_status: None,
            merge_status: None,
        }
    }
    fn with_checkout_sha(mut self, checkout_sha: &str) -> Self {
        self.checkout_sha = Some(checkout_sha.to_owned());
        self
    }
    fn with_status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }
    fn with_approval_status(mut self, approval_status: ApprovalStatus) -> Self {
        self.approval_status = Some(approval_status);
        self
    }
    fn with_merge_status(mut self, merge_status: MergeStatus) -> Self {
        self.merge_status = Some(merge_status);
        self
    }
    fn build(self) -> Result<MergeRequest, ()> {
        if let (Some(checkout_sha), Some(status), Some(approval_status), Some(merge_status)) = (self.checkout_sha, self.status, self.approval_status, self.merge_status) {
            Ok(MergeRequest {
                id: self.id,
                ssh_url: self.ssh_url,
                human_number: self.human_number,
                checkout_sha: checkout_sha,
                status: status,
                approval_status: approval_status,
                merge_status: merge_status,
            })
        } else {
            Err(())
        }
    }
}

fn update_or_create_mr(list: &mut LinkedList<MergeRequest>,
                       id: MrUid,
                       ssh_url: &str,
                       human_number: u64,
                       old_statuses: &[Status],
                       new_checkout_sha: Option<&str>,
                       new_status: Option<Status>,
                       new_approval_status: Option<ApprovalStatus>,
                       new_merge_status: Option<MergeStatus>) {
    if let Some(mut existing_mr) =
        find_mr_mut(&mut *list, id)
    {
        if old_statuses.iter().any(|x| *x == existing_mr.status)
            || old_statuses.len() == 0
        {
            if let Some(new_status) = new_status {
                existing_mr.status = new_status;
            }
            if let Some(new_approval_status) = new_approval_status {
                existing_mr.approval_status = new_approval_status;
            }
            if let Some(new_merge_status) = new_merge_status {
                existing_mr.merge_status = new_merge_status;
            }
            if let Some(new_checkout_sha) = new_checkout_sha {
                existing_mr.checkout_sha = new_checkout_sha.to_owned();
            }
            println!("Updated existing MR: {:?}", existing_mr);
        }
        return;
    }

    let incoming = MergeRequestBuilder::new(id, ssh_url.to_owned(), human_number)
        .with_checkout_sha(new_checkout_sha.unwrap())
        .with_status(new_status.unwrap_or(Status::Open(SubStatusOpen::WaitingForReview)))
        .with_approval_status(new_approval_status.unwrap())
        .with_merge_status(new_merge_status.unwrap())
        .build()
        .unwrap();

    list.push_back(incoming);
    println!("Queued up MR: {:?}", list.back());
}

fn handle_mr(req: &mut Request, queue: &(Mutex<LinkedList<MergeRequest>>, Condvar))
             -> IronResult<Response> {
    println!("handle_mr started            : {}", time::precise_time_ns());
    let &(ref list, _) = queue;
    let ref mut body = req.body;
    let mut s: String = String::new();
    body.read_to_string(&mut s).unwrap();

    println!("{}", req.url);
    println!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    println!("object? {}", json.is_object());
    let object_kind = json.lookup("object_kind").unwrap().as_string().unwrap();
    if object_kind != "merge_request" {
        println!("This endpoint only accepts objects with \"object_kind\":\"merge_request\"");
        return Ok(Response::with(status::Ok));
    }
    let obj = json.as_object().unwrap();
    let attrs = obj.get("object_attributes").unwrap().as_object().unwrap();
    let last_commit = attrs.get("last_commit").unwrap().as_object().unwrap();
    let checkout_sha = last_commit.get("id").unwrap().as_string().unwrap();
    let target_project_id = attrs.get("target_project_id").unwrap().as_u64().unwrap();
    let mr_human_number = attrs.get("iid").unwrap().as_u64().unwrap();
    let mr_id = attrs.get("id").unwrap().as_u64().unwrap();
    let action = json.lookup("object_attributes.action").unwrap().as_string().unwrap();
    let ssh_url = json.lookup("object_attributes.target.ssh_url").unwrap().as_string().unwrap();
    let new_status = match action {
        "open" | "reopen" => Status::Open(SubStatusOpen::WaitingForReview),
        "close" => Status::Closed,
        "merge" => Status::Merged,
        "update" => Status::Open(SubStatusOpen::WaitingForReview),
        _ => panic!("Unexpected MR action: {}", action),
    };

    let merge_status_string = json.lookup("object_attributes.merge_status").unwrap().as_string().unwrap();
    let merge_status = match merge_status_string {
        "can_be_merged" | "unchecked" => MergeStatus::CanBeMerged,
        _ => MergeStatus::CanNotBeMerged,
    };

    {
        let mut list = list.lock().unwrap();
        if let Some(mut existing_mr) =
            find_mr_mut(
                &mut *list,
                MrUid { target_project_id: target_project_id, id: mr_id })
        {
            existing_mr.status = new_status;
            existing_mr.approval_status = ApprovalStatus::Pending;
            existing_mr.merge_status = merge_status;
            existing_mr.checkout_sha = checkout_sha.to_string();
            println!("Updated existing MR");
            return Ok(Response::with(status::Ok));
        }
        let incoming = MergeRequest {
            id: MrUid { target_project_id: target_project_id, id: mr_id },
            ssh_url: ssh_url.to_owned(),
            checkout_sha: checkout_sha.to_owned(),
            status: new_status,
            human_number: mr_human_number,
            approval_status: ApprovalStatus::Pending,
            merge_status: merge_status,
        };
        list.push_back(incoming);
        println!("Queued up...");
    }

    println!("handle_mr finished           : {}", time::precise_time_ns());
    return Ok(Response::with(status::Ok));
}

fn handle_comment(req: &mut Request, queue: &(Mutex<LinkedList<MergeRequest>>, Condvar),
                  config: &toml::Value)
                  -> IronResult<Response> {
    println!("handle_comment started       : {}", time::precise_time_ns());

    println!("{}", req.url);
    let mut s: String = String::new();
    let ref mut body = req.body;
    body.read_to_string(&mut s).unwrap();
    println!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    println!("object? {}", json.is_object());
    let object_kind = json.lookup("object_kind").unwrap().as_string().unwrap();
    if object_kind != "note" {
        println!("This endpoint only accepts objects with \"object_kind\":\"note\"");
        return Ok(Response::with(status::Ok));
    }
    let obj = json.as_object().unwrap();
    let user = obj.get("user").unwrap().as_object().unwrap();
    let username = user.get("username").unwrap().as_string().unwrap();

    let reviewers = config.lookup("gitlab.reviewers").unwrap().as_slice().unwrap();
    let is_comment_author_reviewer = reviewers.iter().any(|s| s.as_str().unwrap() == username);

    if is_comment_author_reviewer {
        let &(ref list, ref cvar) = queue;

        let last_commit_id = json.lookup("merge_request.last_commit.id").unwrap().as_string().unwrap().to_owned();
        let target_project_id = json.lookup("merge_request.target_project_id").unwrap().as_u64().unwrap();
        let mr_human_number = json.lookup("merge_request.iid").unwrap().as_u64().unwrap();
        let mr_id = json.lookup("merge_request.id").unwrap().as_u64().unwrap();
        let state = json.lookup("merge_request.state").unwrap().as_string().unwrap();
        let ssh_url = json.lookup("merge_request.target.ssh_url").unwrap().as_string().unwrap();
        // This is unused as we only handle comments that are issued by reviewer
        let new_status = match state {
            "opened" | "reopened" | "updated" => Status::Open(SubStatusOpen::WaitingForReview),
            "closed" => Status::Closed,
            "merged" => Status::Merged,
            _ => panic!("Unexpected MR state: {}", state),
        };
        let merge_status_string = json.lookup("merge_request.merge_status").unwrap().as_string().unwrap();
        let merge_status = match merge_status_string {
            "can_be_merged" | "unchecked" => MergeStatus::CanBeMerged,
            _ => MergeStatus::CanNotBeMerged,
        };

        let attrs = obj.get("object_attributes").unwrap().as_object().unwrap();
        let note = attrs.get("note").unwrap().as_string().unwrap();

        let mention = "@shurik ";
        let mention_len = mention.len();
        if &note[0..mention_len] == mention {
            match &note[mention_len..] {
                "r+" | "одобряю" => {
                    let mut list = list.lock().unwrap();
                    update_or_create_mr(
                        &mut *list,
                        MrUid { target_project_id: target_project_id, id: mr_id },
                        ssh_url,
                        mr_human_number,
                        &[Status::Open(SubStatusOpen::WaitingForReview)][..],
                        Some(&last_commit_id),
                        Some(Status::Open(SubStatusOpen::WaitingForCi)),
                        Some(ApprovalStatus::Approved),
                        Some(merge_status),
                        );
                    cvar.notify_one();
                    println!("Notified...");
                },
                "r-" | "отказываю" => {
                    let mut list = list.lock().unwrap();
                    update_or_create_mr(
                        &mut *list,
                        MrUid { target_project_id: target_project_id, id: mr_id },
                        ssh_url,
                        mr_human_number,
                        &[
                            Status::Open(SubStatusOpen::WaitingForCi),
                            Status::Open(SubStatusOpen::WaitingForMerge)][..],
                        Some(&last_commit_id),
                        Some(Status::Open(SubStatusOpen::WaitingForReview)),
                        Some(ApprovalStatus::Rejected),
                        Some(merge_status),
                        );
                },
                "try" | "попробуй" => {
                    let mut list = list.lock().unwrap();
                    update_or_create_mr(
                        &mut *list,
                        MrUid { target_project_id: target_project_id, id: mr_id },
                        ssh_url,
                        mr_human_number,
                        &[],
                        Some(&last_commit_id),
                        Some(Status::Open(SubStatusOpen::WaitingForCi)),
                        None,
                        Some(merge_status),
                        );
                    cvar.notify_one();
                    println!("Notified...");
                }
                _ => {}
            }
        }
    } else {
        println!("Comment author {} is not reviewer. Reviewers: {:?}", username, reviewers);
    }

    println!("handle_comment finished      : {}", time::precise_time_ns());
    return Ok(Response::with(status::Ok));
}

fn handle_build_request(queue: &(Mutex<LinkedList<MergeRequest>>, Condvar), config: &toml::Value) -> !
{
    println!("handle_build_request started : {}", time::precise_time_ns());
    let gitlab_user = config.lookup("gitlab.user").unwrap().as_str().unwrap();
    let gitlab_password = config.lookup("gitlab.password").unwrap().as_str().unwrap();
    let gitlab_api_root = config.lookup("gitlab.url").unwrap().as_str().unwrap();

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
    println!("Logged in to GitLab, private_token == {}", private_token);

    loop {
        println!("handle_build_request iterated: {}", time::precise_time_ns());

        let &(ref mutex, ref cvar) = queue;
        println!("Waiting to get the request...");
        let mut list = mutex.lock().unwrap();
        while list.is_empty() {
            list = cvar.wait(list).unwrap();
        }
        println!("{:?}", &*list);
        let mut request = list.pop_front().unwrap();
        println!("Got the request: {:?}", request);
        let arg = request.checkout_sha.clone();
        let mr_id = request.id;
        let mr_human_number = request.human_number;
        let request_status = request.status;
        let merge_status = request.merge_status;
        let ssh_url = request.ssh_url.clone();
        println!("{:?}", request.status);
        if merge_status != MergeStatus::CanBeMerged {
            let message = &*format!("{{ \"note\": \":umbrella: в результате изменений целевой ветки, этот MR больше нельзя слить. Пожалуйста, обновите его (rebase или merge)\"}}");
            gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
            continue;
        }
        if request_status != Status::Open(SubStatusOpen::WaitingForCi) {
            continue;
        }
        drop(list);

        let message = &*format!("{{ \"note\": \":hourglass: проверяю коммит #{}\"}}", arg);
        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);

        git::set_remote_url(&ssh_url);
        git::set_user("Shurik", "shurik@example.com");
        git::fetch();
        git::checkout("master");
        git::reset_hard("origin/master");
        git::checkout("try");
        git::reset_hard(&arg);
        match git::rebase("master") {
            Ok(_) => {},
            Err(_) => {
                let message = &*format!("{{ \"note\": \":umbrella: не удалось сделать rebase MR на master. Пожалуйста, обновите его (rebase или merge)\"}}");
                gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                continue;
            }
        }
        git::push(true);

        let http_user = config.lookup("jenkins.user").unwrap().as_str().unwrap();
        let http_password = config.lookup("jenkins.password").unwrap().as_str().unwrap();
        let token = config.lookup("jenkins.token").unwrap().as_str().unwrap();
        let jenkins_job_url = config.lookup("jenkins.job-url").unwrap().as_str().unwrap();
        println!("{} {} {} {}", http_user, http_password, token, jenkins_job_url);

        let queue_url = jenkins::enqueue_build(http_user, http_password, jenkins_job_url, token);
        println!("Parsed the location == {}", queue_url);

        let build_url = jenkins::poll_queue(http_user, http_password, &queue_url);

        let result_string = jenkins::poll_build(http_user, http_password, &build_url);

        println!("{}", result_string);
        if result_string == "SUCCESS" {
            let &(ref mutex, _) = queue;
            let list = &mut *mutex.lock().unwrap();
            if let Some(new_request) = find_mr_mut(list, mr_id)
            {
                if new_request.checkout_sha == request.checkout_sha {
                    if new_request.approval_status != ApprovalStatus::Approved {
                        println!("The MR was rejected in the meantime, not merging");
                        let message = &*format!("{{ \"note\": \":no_entry_sign: пока мы тестировали, коммит запретили. Не сливаю\"}}");
                        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                        continue;
                    } else {
                        println!("MR has new comments, and head is same commit, so it doesn't make sense to account for changes");
                    }
                } else {
                    println!("MR head is different commit, not merging");
                    let message = &*format!("{{ \"note\": \":no_entry_sign: пока мы тестировали, MR обновился. Не сливаю\"}}");
                    gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                    continue;
                }
            }
            assert_eq!(request.status, Status::Open(SubStatusOpen::WaitingForCi));
            if request.approval_status == ApprovalStatus::Approved {
                println!("Merging");
                let message = &*format!("{{ \"note\": \":sunny: тесты прошли, сливаю\"}}");
                gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                request.status = Status::Open(SubStatusOpen::WaitingForMerge);
                git::checkout("master");
                match git::merge(mr_human_number) {
                    Ok(_) => {},
                    Err(_) => {
                        let message = &*format!("{{ \"note\": \":umbrella: не смог слить MR. Пожалуйста, обновите его (rebase или merge)\"}}");
                        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                        continue;
                    },
                }
                git::status();
                git::push(false);
                request.status = Status::Merged;
                println!("Updated existing MR");
                let message = &*format!("{{ \"note\": \":ok_hand: успешно\"}}");
                gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                continue;
            }
        }
        let &(ref mutex, _) = queue;
        let list = &mut *mutex.lock().unwrap();
        let mut need_push_back = false;
        {
            if let Some(new_request) = find_mr_mut(list, mr_id)
            {
                if new_request.checkout_sha != request.checkout_sha
                    || new_request.approval_status != request.approval_status
                {
                    println!("The MR was updated, discarding this one");
                } else {
                    println!("Pushing back old MR");
                    request.status = Status::Open(SubStatusOpen::WaitingForReview);
                    need_push_back = true;
                }
            } else {
                println!("Push back old MR and setting it for review");
                request.status = Status::Open(SubStatusOpen::WaitingForReview);
                need_push_back = true;
            }
        }
        let mr_id = request.id.clone();
        if need_push_back {
            list.push_back(request);
        }
        println!("{:?}", &*list);

        let build_result_message = match &*result_string {
            "SUCCESS" => "успешно",
            _ => "с ошибками",
        };
        let indicator = match &*result_string {
            "SUCCESS" => ":sunny:",
            _ => ":bangbang:",
        };

        let message = &*format!("{{ \"note\": \"{} тестирование завершено [{}]({})\"}}", indicator, build_result_message, build_url);

        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);

        drop(list);
        println!("handle_build_request finished: {}", time::precise_time_ns());
    }
    println!("At: {}", time::precise_time_ns());
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
    let value: toml::Value = toml::Value::Table(parser.parse().unwrap());
    let config: Arc<toml::Value> = Arc::new(value);

    println!("Dry run: {}", matches.is_present("dry-run"));
    let gitlab_port =
        matches
        .value_of("GITLAB_PORT")
        .unwrap_or(
            config.lookup("gitlab.webhook-port")
                .unwrap_or(
                    &toml::Value::String("10042".to_owned()))
                .as_str()
                .unwrap())
        .parse().unwrap_or(10042);
    let default_address = toml::Value::String("localhost".to_owned());
    let gitlab_address =
        matches
        .value_of("GITLAB_ADDRESS")
        .unwrap_or(
            config.lookup("gitlab.webhook-address")
                .unwrap_or(
                    &default_address)
                .as_str()
                .unwrap())
        .to_owned();
    println!("GitLab port: {}", gitlab_port);
    println!("GitLab address: {}", gitlab_address);

    let queue = Arc::new((Mutex::new(LinkedList::new()), Condvar::new()));
    let queue2 = queue.clone();
    let queue3 = queue.clone();
    let config2 = config.clone();

    let builder = thread::spawn(move || {
        handle_build_request(&*queue, &*config);
    });

    let mut router = router::Router::new();
    router.post("/api/v1/mr",
                move |req: &mut Request|
                handle_mr(req, &*queue2));
    router.post("/api/v1/comment",
                move |req: &mut Request|
                handle_comment(req, &*queue3, &*config2));
    Iron::new(router).http(
        (&*gitlab_address, gitlab_port))
        .expect("Couldn't start the web server");
    builder.join().unwrap();
}
