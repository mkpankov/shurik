extern crate clap;
extern crate hyper;
extern crate iron;

#[macro_use]
extern crate log;
extern crate env_logger;

extern crate regex;
extern crate router;
extern crate serde_json;
extern crate time;
extern crate toml;

use clap::App;
use hyper::Client;
use iron::*;

use std::collections::{LinkedList, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
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

#[derive(Debug, Clone)]
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
            debug!("{:?}", i);
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
            debug!("{:?}", i);
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
            info!("Updated existing MR: {:?}", existing_mr);
        }
        return;
    }

    let incoming = MergeRequestBuilder::new(id, ssh_url.to_owned(), human_number)
        .with_checkout_sha(new_checkout_sha.unwrap())
        .with_status(new_status.unwrap_or(Status::Open(SubStatusOpen::WaitingForReview)))
        .with_approval_status(new_approval_status.unwrap())
        .with_merge_status(new_merge_status.unwrap_or(MergeStatus::CanBeMerged))
        .build()
        .unwrap();

    list.push_back(incoming);
    info!("Queued up MR: {:?}", list.back());
}

fn handle_mr(req: &mut Request, queue: &(Mutex<LinkedList<MergeRequest>>, Condvar))
             -> IronResult<Response> {
    debug!("handle_mr started            : {}", time::precise_time_ns());
    let &(ref list, _) = queue;
    let ref mut body = req.body;
    let mut s: String = String::new();
    body.read_to_string(&mut s).unwrap();

    debug!("{}", req.url);
    debug!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    debug!("object? {}", json.is_object());
    let object_kind = json.lookup("object_kind").unwrap().as_string().unwrap();
    if object_kind != "merge_request" {
        error!("This endpoint only accepts objects with \"object_kind\":\"merge_request\"");
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
        update_or_create_mr(
            &mut *list,
            MrUid { target_project_id: target_project_id, id: mr_id },
            ssh_url,
            mr_human_number,
            &[],
            Some(checkout_sha),
            Some(new_status),
            Some(ApprovalStatus::Pending),
            Some(merge_status)
            );
    }

    debug!("handle_mr finished           : {}", time::precise_time_ns());
    return Ok(Response::with(status::Ok));
}

fn handle_comment(req: &mut Request, queue: &(Mutex<LinkedList<MergeRequest>>, Condvar),
                  config: &toml::Value)
                  -> IronResult<Response> {
    debug!("handle_comment started       : {}", time::precise_time_ns());

    debug!("{}", req.url);
    let mut s: String = String::new();
    let ref mut body = req.body;
    body.read_to_string(&mut s).unwrap();
    debug!("{}", s);

    let json: serde_json::value::Value = serde_json::from_str(&s).unwrap();
    debug!("object? {}", json.is_object());
    let object_kind = json.lookup("object_kind").unwrap().as_string().unwrap();
    if object_kind != "note" {
        error!("This endpoint only accepts objects with \"object_kind\":\"note\"");
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
        let ssh_url = json.lookup("merge_request.target.ssh_url").unwrap().as_string().unwrap();
        // This is unused as we only handle comments that are issued by reviewer

        let attrs = obj.get("object_attributes").unwrap().as_object().unwrap();
        let note = attrs.get("note").unwrap().as_string().unwrap();
        let mut old_statuses = Vec::new();
        let mut new_status = None;
        let mut new_approval_status = None;
        let mut needs_notification = false;

        let mention = "@shurik ";
        let mention_len = mention.len();
        if note.len() >= mention.len() {
            if &note[0..mention_len] == mention {
                match &note[mention_len..] {
                    "r+" | "одобряю" => {
                        old_statuses.push(Status::Open(SubStatusOpen::WaitingForReview));
                        new_status = Some(Status::Open(SubStatusOpen::WaitingForCi));
                        new_approval_status = Some(ApprovalStatus::Approved);
                        needs_notification = true;
                    },
                    "r-" | "отказываю" => {
                        old_statuses.extend(
                            &[Status::Open(SubStatusOpen::WaitingForCi),
                              Status::Open(SubStatusOpen::WaitingForMerge)]);
                        new_status = Some(Status::Open(SubStatusOpen::WaitingForReview));
                        new_approval_status = Some(ApprovalStatus::Rejected);
                        ;
                    },
                    "try" | "попробуй" => {
                        new_status = Some(Status::Open(SubStatusOpen::WaitingForCi));
                        needs_notification = true;
                    },
                    _ => {
                        warn!("Unknown command: {}", &note[mention_len..]);
                        return Ok(Response::with(status::Ok));
                    }
                }
            }
        }
        update_or_create_mr(
            &mut *list.lock().unwrap(),
            MrUid { target_project_id: target_project_id, id: mr_id },
            ssh_url,
            mr_human_number,
            &old_statuses,
            Some(&last_commit_id),
            new_status,
            new_approval_status,
            None,
            );
        if needs_notification {
            cvar.notify_one();
            info!("Notified...");
        }
    } else {
        info!("Comment author {} is not reviewer. Reviewers: {:?}", username, reviewers);
    }

    debug!("handle_comment finished      : {}", time::precise_time_ns());
    return Ok(Response::with(status::Ok));
}

fn handle_build_request(queue: &(Mutex<LinkedList<MergeRequest>>, Condvar), config: &toml::Value) -> !
{
    debug!("handle_build_request started : {}", time::precise_time_ns());
    let gitlab_user = config.lookup("gitlab.user").unwrap().as_str().unwrap();
    let gitlab_password = config.lookup("gitlab.password").unwrap().as_str().unwrap();
    let gitlab_api_root = config.lookup("gitlab.url").unwrap().as_str().unwrap();
    let workspace_dir = config.lookup("general.workspace-dir").unwrap().as_str().unwrap();
    let key_path = config.lookup("gitlab.ssh-key-path").unwrap().as_str().unwrap();

    let client = Client::new();

    let mut res = client.post(&*format!("{}/session", gitlab_api_root))
        .body(&*format!("login={}&password={}", gitlab_user, gitlab_password))
        .send()
        .unwrap();
    assert_eq!(res.status, hyper::status::StatusCode::Created);
    let mut text = String::new();
    res.read_to_string(&mut text).unwrap();
    let json: serde_json::value::Value = serde_json::from_str(&text).unwrap();
    debug!("object? {}", json.is_object());
    let obj = json.as_object().unwrap();
    let private_token = obj.get("private_token").unwrap().as_string().unwrap();
    info!("Logged in to GitLab");
    debug!("private_token == {}", private_token);

    loop {
        debug!("handle_build_request iterated: {}", time::precise_time_ns());

        let &(ref mutex, ref cvar) = queue;
        info!("Waiting to get the request...");
        let mut list = mutex.lock().unwrap();
        while list.is_empty() {
            list = cvar.wait(list).unwrap();
        }
        info!("{:?}", &*list);

        let mut request = list.pop_front().unwrap();
        info!("Got the request: {:?}", request);

        let list_copy = list.clone();
        debug!("List copy: {:?}", list_copy);

        let arg = request.checkout_sha.clone();
        let mr_id = request.id;
        let mr_human_number = request.human_number;
        let request_status = request.status;
        let ssh_url = request.ssh_url.clone();
        debug!("{:?}", request.status);

        if request_status != Status::Open(SubStatusOpen::WaitingForCi) {
            continue;
        }
        drop(list);

        let message = &*format!("{{ \"note\": \":hourglass: проверяю коммит #{}\"}}", arg);
        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);

        git::set_remote_url(workspace_dir, &ssh_url);
        git::set_user(workspace_dir, "Shurik", "shurik@example.com");
        git::fetch(workspace_dir, key_path);
        git::reset_hard(workspace_dir, None);

        git::checkout(workspace_dir, "master");
        git::reset_hard(workspace_dir, Some("origin/master"));
        git::checkout(workspace_dir, "try");
        git::reset_hard(workspace_dir, Some(&arg));
        match git::merge(workspace_dir, "master", mr_human_number, false) {
            Ok(_) => {},
            Err(_) => {
                let message = &*format!("{{ \"note\": \":umbrella: не удалось слить master в MR. Пожалуйста, обновите его (rebase или merge)\"}}");
                gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                continue;
            }
        }
        git::push(workspace_dir, key_path, true);

        let http_user = config.lookup("jenkins.user").unwrap().as_str().unwrap();
        let http_password = config.lookup("jenkins.password").unwrap().as_str().unwrap();
        let token = config.lookup("jenkins.token").unwrap().as_str().unwrap();
        let jenkins_job_url = config.lookup("jenkins.job-url").unwrap().as_str().unwrap();
        debug!("{} {} {} {}", http_user, http_password, token, jenkins_job_url);

        let queue_url = jenkins::enqueue_build(http_user, http_password, jenkins_job_url, token);
        info!("Queue item URL: {}", queue_url);

        let build_url = jenkins::poll_queue(http_user, http_password, &queue_url);
        info!("Build job URL: {}", queue_url);

        let result_string = jenkins::poll_build(http_user, http_password, &build_url);
        info!("Result: {}", result_string);

        if result_string == "SUCCESS" {
            let &(ref mutex, _) = queue;
            let list = &mut *mutex.lock().unwrap();
            if let Some(new_request) = find_mr_mut(list, mr_id)
            {
                if new_request.checkout_sha == request.checkout_sha {
                    if new_request.approval_status != ApprovalStatus::Approved {
                        info!("The MR was rejected in the meantime, not merging");
                        let message = &*format!("{{ \"note\": \":no_entry_sign: пока мы тестировали, коммит запретили. Не сливаю\"}}");
                        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                        continue;
                    } else {
                        info!("MR has new comments, and head is same commit, so it doesn't make sense to account for changes");
                    }
                } else {
                    info!("MR head is different commit, not merging");
                    let message = &*format!("{{ \"note\": \":no_entry_sign: пока мы тестировали, MR обновился. Не сливаю\"}}");
                    gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                    continue;
                }
            }
            drop(list);
            assert_eq!(request.status, Status::Open(SubStatusOpen::WaitingForCi));
            if request.approval_status == ApprovalStatus::Approved {
                info!("Merging");
                let message = &*format!("{{ \"note\": \":sunny: тесты прошли, сливаю\"}}");
                gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                request.status = Status::Open(SubStatusOpen::WaitingForMerge);
                git::checkout(workspace_dir, "master");
                match git::merge(workspace_dir, "try", mr_human_number, true) {
                    Ok(_) => {},
                    Err(_) => {
                        let message = &*format!("{{ \"note\": \":umbrella: не смог слить MR. Пожалуйста, обновите его (rebase или merge)\"}}");
                        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
                        continue;
                    },
                }
                git::status(workspace_dir);
                git::push(workspace_dir, key_path, false);
                request.status = Status::Merged;
                info!("Updated existing MR");
                let message = &*format!("{{ \"note\": \":ok_hand: успешно\"}}");
                gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);

                for mr in list_copy.iter() {
                    info!("MR to try merge: {:?}", mr);
                    git::set_remote_url(workspace_dir, &ssh_url);
                    git::set_user(workspace_dir, "Shurik", "shurik@example.com");
                    mr_try_merge_and_report_if_impossible(mr, gitlab_api_root, private_token, workspace_dir, key_path);
                }
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
                    info!("The MR was updated, discarding this one");
                } else {
                    info!("Pushing back old MR");
                    request.status = Status::Open(SubStatusOpen::WaitingForReview);
                    need_push_back = true;
                }
            } else {
                info!("Push back old MR and setting it for review");
                request.status = Status::Open(SubStatusOpen::WaitingForReview);
                need_push_back = true;
            }
        }
        let mr_id = request.id.clone();
        if need_push_back {
            list.push_back(request);
        }
        info!("{:?}", &*list);
        drop(list);

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

        debug!("handle_build_request finished: {}", time::precise_time_ns());
    }
}

fn mr_try_merge_and_report_if_impossible(mr: &MergeRequest,
                                         gitlab_api_root: &str,
                                         private_token: &str,
                                         workspace_dir: &str,
                                         key_path: &str)
{
    info!("Got the mr: {:?}", mr);
    let arg = mr.checkout_sha.clone();
    let mr_id = mr.id;
    let mr_human_number = mr.human_number;
    let merge_status = mr.merge_status;
    debug!("{:?}", mr.status);
    if merge_status != MergeStatus::CanBeMerged {
        let message = &*format!("{{ \"note\": \":umbrella: в результате изменений целевой ветки, этот MR больше нельзя слить. Пожалуйста, обновите его (rebase или merge). Проверенный коммит: #{}\"}}", arg);
        gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
        return;
    }
    git::reset_hard(workspace_dir, None);

    git::checkout(workspace_dir, "master");
    git::reset_hard(workspace_dir, Some("origin/master"));
    git::checkout(workspace_dir, "try");
    git::fetch(workspace_dir, key_path);
    git::reset_hard(workspace_dir, Some(&arg));
    match git::merge(workspace_dir, "master", mr_human_number, false) {
        Ok(_) => {},
        Err(_) => {
            let message = &*format!("{{ \"note\": \":umbrella: не удалось слить master в MR. Пожалуйста, обновите его (rebase или merge). Проверенный коммит: #{}\"}}", arg);
            gitlab::post_comment(gitlab_api_root, private_token, mr_id, message);
            return;
        }
    }
}

fn main() {
    env_logger::init().unwrap();

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

    debug!("Dry run: {}", matches.is_present("dry-run"));
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
    info!("GitLab port: {}", gitlab_port);
    info!("GitLab address: {}", gitlab_address);

    let queue = Arc::new((Mutex::new(LinkedList::new()), Condvar::new()));
    let queue2 = queue.clone();
    let config2 = config.clone();
    let config3 = config.clone();

    let builder = thread::spawn(move || {
        handle_build_request(&*queue, &*config);
    });
    let mut projects = HashMap::new();
    for project_toml in config3.lookup("project").unwrap().as_slice().unwrap() {
        let key = project_toml.lookup("id").unwrap().as_integer().unwrap();
        let toml_slice = project_toml.lookup("reviewers").unwrap().as_slice().unwrap();
        let str_vec: Vec<&str> = toml_slice.iter().map(|x| x.as_str().unwrap()).collect();
        let string_vec: Vec<String> = str_vec.iter().map(|x: &&str| -> String { (*x).to_owned() }).collect();
        let p = Project {
            id: key,
            workspace_dir: PathBuf::from(project_toml.lookup("workspace-dir").unwrap().as_str().unwrap()),
            reviewers: string_vec,
        };
        projects.insert(key, p);
    }
    println!("{:?}", projects);

    let mut router = router::Router::new();
    for id in projects.keys() {
        let queue2 = queue2.clone();
        let queue3 = queue2.clone();
        let config2 = config2.clone();
        router.post(format!("/api/v1/{}/mr", id),
                    move |req: &mut Request|
                    handle_mr(req, &*queue2));
        router.post(format!("/api/v1/{}/comment", id),
                    move |req: &mut Request|
                    handle_comment(req, &*queue3, &*config2));
    }

    Iron::new(router).http(
        (&*gitlab_address, gitlab_port))
        .expect("Couldn't start the web server");
    builder.join().unwrap();
}

#[derive(Debug)]
struct Project {
    id: i64,
    workspace_dir: PathBuf,
    reviewers: Vec<String>,
}
