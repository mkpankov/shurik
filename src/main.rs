extern crate clap;
extern crate hyper;
extern crate iron;

#[macro_use]
extern crate log;
extern crate env_logger;

#[macro_use]
extern crate quick_error;
extern crate regex;
extern crate router;
extern crate rustc_serialize;
extern crate serde_json;
extern crate time;
extern crate toml;
extern crate url;

use clap::App;
use iron::*;
use rustc_serialize::{Encodable, Decodable, Encoder, Decoder};
use rustc_serialize::json::{self};

use std::collections::{LinkedList, HashMap};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

mod git;
mod jenkins;
mod gitlab;

#[derive(RustcDecodable, RustcEncodable)]
#[derive(Debug, PartialEq, Eq, Clone)]
enum SubStatusBuilding {
    NotStarted,
    Queued(String),
    InProgress(String),
    Finished(String, String),
}

#[derive(RustcDecodable, RustcEncodable)]
#[derive(Debug, PartialEq, Eq, Clone)]
enum SubStatusUpdating {
    NotStarted,
    InProgress,
    Finished,
}

#[derive(RustcDecodable, RustcEncodable)]
#[derive(Debug, PartialEq, Eq, Clone)]
enum SubStatusOpen {
    WaitingForReview,
    Updating(SubStatusUpdating),
    WaitingForCi,
    Building(SubStatusBuilding),
    WaitingForResultDispatch(Option<String>, Option<String>),
}

#[derive(RustcDecodable, RustcEncodable)]
#[derive(Debug, PartialEq, Eq, Clone)]
enum Status {
    Open(SubStatusOpen),
    Merged,
    Closed,
}

#[derive(RustcDecodable, RustcEncodable)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(RustcDecodable, RustcEncodable)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum MergeStatus {
    CanBeMerged,
    CanNotBeMerged,
}

#[derive(RustcDecodable, RustcEncodable)]
#[derive(Debug, Clone)]
struct MergeRequest {
    id: MrUid,
    human_number: u64,
    checkout_sha: String,
    status: Status,
    approval_status: ApprovalStatus,
    merge_status: MergeStatus,
}

#[derive(Debug)]
struct WorkerTask {
    id: MrUid,
    job_type: JobType,
    approval_status: ApprovalStatus,
}

#[derive(Debug, PartialEq, Eq)]
enum JobType {
    Try,
    Merge
}

#[derive(Debug)]
struct Project {
    id: u64,
    name: String,
    workspace_dir: PathBuf,
    reviewers: Vec<String>,
    job_url: String,
    ssh_url: Mutex<String>,
}

#[derive(Debug)]
struct ProjectSet {
    name: String,
    projects: HashMap<u64, Project>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MrUid {
    id: u64,
    target_project_id: u64,
}

impl Encodable for MrUid {
    fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
        format!("{},{}", self.id, self.target_project_id).encode(s)
    }
}

impl Decodable for MrUid {
    fn decode<D: Decoder>(d: &mut D) -> Result<Self, D::Error> {
        let s = try!(d.read_str());
        let s_v: Vec<_> = s.split(",").collect();
        let mut v: Vec<u64> = s_v.iter().map(|x| x.parse().unwrap()).collect();
        let mr_uid = MrUid {
            target_project_id: v.pop().unwrap(),
            id: v.pop().unwrap(),
        };
        Ok(mr_uid)
    }
}

fn update_or_create_mr(
    storage: &mut HashMap<MrUid, MergeRequest>,
    id: MrUid,
    human_number: u64,
    old_statuses: &[Status],
    new_checkout_sha: Option<&str>,
    new_status: Option<Status>,
    new_approval_status: Option<ApprovalStatus>,
    new_merge_status: Option<MergeStatus>) {
    if let Some(mut existing_mr) = storage.get_mut(&id) {
        let is_waiting_for_result_dispatch_acceptable = |s| {
            if let &Status::Open(SubStatusOpen::WaitingForResultDispatch(_, _)) = s {
                true
            } else {
                false
            }
        };
        if old_statuses.iter().any(|x| *x == existing_mr.status || is_waiting_for_result_dispatch_acceptable(x))
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

    let incoming = MergeRequest {
        id: id,
        human_number: human_number,
        checkout_sha: new_checkout_sha.unwrap().to_owned(),
        status: new_status.unwrap_or(Status::Open(SubStatusOpen::WaitingForReview)),
        approval_status: new_approval_status.unwrap_or(ApprovalStatus::Pending),
        merge_status: new_merge_status.unwrap_or(MergeStatus::CanBeMerged),
    };

    storage.insert(id, incoming);
    info!("Added MR: {:?}", storage[&id]);
}

fn save_state(
    state_save_dir: &str,
    name: &str,
    mr_storage: &Mutex<HashMap<MrUid, MergeRequest>>)
{
    let serialized;
    {
        let mr_storage = &*mr_storage.lock().unwrap();
        let maybe_serialized = json::encode(mr_storage);
        match maybe_serialized {
            Ok(s) => serialized = s,
            Err(e) => panic!("Couldn't encode state to JSON: {}", e),
        }
    }
    let mut path = PathBuf::from(state_save_dir);
    path.push(name);
    path.set_extension("state.json");
    let mut file = File::create(path).unwrap();
    file.write_all(serialized.as_bytes()).unwrap();
}

fn load_state(
    state_save_dir: &str,
    name: &str)
    -> HashMap<MrUid, MergeRequest>
{
    let mut path = PathBuf::from(state_save_dir);
    path.push(name);
    path.set_extension("state.json");
    if let Ok(mut file) = File::open(path) {
        let mut serialized = String::new();
        file.read_to_string(&mut serialized).unwrap();
        let mr_storage: HashMap<MrUid, MergeRequest> = json::decode(&serialized).unwrap();
        mr_storage
    } else {
        HashMap::new()
    }
}

fn handle_mr(
    req: &mut Request,
    mr_storage: &Mutex<HashMap<MrUid, MergeRequest>>,
    project_set: &ProjectSet,
    state_save_dir: &str)
    -> IronResult<Response> {
    debug!("handle_mr started            : {}", time::precise_time_ns());
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
    let project_id = json.lookup("object_attributes.target_project_id").unwrap().as_u64().unwrap();

    let projects = &project_set.projects;
    let mut project_ids_iter = projects.keys();
    if ! project_ids_iter.any(|x| *x as u64 == project_id) {
        let project_ids: Vec<_> = projects.keys().collect();
        error!("Project id mismatch. Handler is setup for projects {:?}, but webhook info has target_project_id {}", project_ids, project_id);
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

    let project = &project_set.projects[&project_id];
    {
        let mut ssh_url_ref = project.ssh_url.lock().unwrap();
        *ssh_url_ref = ssh_url.to_owned();
    }

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

    let id = MrUid { target_project_id: target_project_id, id: mr_id };
    if let Status::Open(_) = new_status {
        ;
    } else {
        mr_storage.lock().unwrap().remove(&id);
    }
    {
        update_or_create_mr(
            &mut *mr_storage.lock().unwrap(),
            id,
            mr_human_number,
            &[],
            Some(checkout_sha),
            Some(new_status),
            Some(ApprovalStatus::Pending),
            Some(merge_status)
            );
    }

    save_state(state_save_dir, &project_set.name, mr_storage);
    debug!("handle_mr finished           : {}", time::precise_time_ns());
    Ok(Response::with(status::Ok))
}

fn handle_comment(
    req: &mut Request,
    mr_storage: &Mutex<HashMap<MrUid, MergeRequest>>,
    worker_queue: &(Mutex<LinkedList<WorkerTask>>, Condvar),
    project_set: &ProjectSet,
    state_save_dir: &str)
    -> IronResult<Response> {
    debug!("handle_comment started       : {}", time::precise_time_ns());
    info!("This thread handles projects: {:?}", project_set);

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

    let project_id = json.lookup("object_attributes.project_id").unwrap().as_u64().unwrap();
    let projects = &project_set.projects;
    let project = &projects[&project_id];
    let reviewers = &project.reviewers;

    let is_comment_author_self = "shurik" == username;

    if is_comment_author_self {
        info!("Got comment from myself ({}), not handling.", username);
        return Ok(Response::with(status::Ok));
    }

    let is_comment_author_reviewer = reviewers.iter().any(|s| s == username);

    let last_commit_id = json.lookup("merge_request.last_commit.id").unwrap().as_string().unwrap().to_owned();
    let target_project_id = json.lookup("merge_request.target_project_id").unwrap().as_u64().unwrap();
    let mr_human_number = json.lookup("merge_request.iid").unwrap().as_u64().unwrap();
    let mr_id = json.lookup("merge_request.id").unwrap().as_u64().unwrap();
    let ssh_url = json.lookup("merge_request.target.ssh_url").unwrap().as_string().unwrap();

    let title = json.lookup("merge_request.title").unwrap().as_string().unwrap();
    info!("{}", title);

    let attrs = obj.get("object_attributes").unwrap().as_object().unwrap();
    let note = attrs.get("note").unwrap().as_string().unwrap();

    let mut old_statuses = Vec::new();
    let mut new_status = None;
    let mut new_approval_status = None;
    let mut needs_notification = false;

    let mention = "@shurik ";
    let mention_len = mention.len();
    if note.starts_with(mention) {
        // SAFE: If note starts with mention, this is safe:
        // mention_len points to character boundary
        match &note[mention_len..] {
            "r+" | "одобряю" => {
                if is_comment_author_reviewer {
                    old_statuses.push(Status::Open(SubStatusOpen::WaitingForReview));
                    new_status = Some(Status::Open(SubStatusOpen::Updating(SubStatusUpdating::NotStarted)));
                    new_approval_status = Some(ApprovalStatus::Approved);
                    needs_notification = true;
                } else {
                    info!("Comment author {} is not reviewer. Reviewers: {:?}", username, reviewers);
                    return Ok(Response::with(status::Ok));
                }
            },
            "r-" | "отказываю" => {
                if is_comment_author_reviewer {
                    old_statuses.push(Status::Open(SubStatusOpen::WaitingForCi));
                    old_statuses.push(Status::Open(SubStatusOpen::WaitingForResultDispatch(None, None)));
                    old_statuses.push(Status::Open(SubStatusOpen::Updating(SubStatusUpdating::NotStarted)));
                    new_status = Some(Status::Open(SubStatusOpen::WaitingForReview));
                    new_approval_status = Some(ApprovalStatus::Rejected);
                } else {
                    info!("Comment author {} is not reviewer. Reviewers: {:?}", username, reviewers);
                    return Ok(Response::with(status::Ok));
                }
                ;
            },
            "try" | "попробуй" => {
                new_status = Some(Status::Open(SubStatusOpen::Updating(SubStatusUpdating::NotStarted)));
                needs_notification = true;
            },
            _ => {
                warn!("Unknown command: {}", &note[mention_len..]);
                return Ok(Response::with(status::Ok));
            }
        }
    }
    {
        let id = MrUid { target_project_id: target_project_id, id: mr_id };
        update_or_create_mr(
            &mut *mr_storage.lock().unwrap(),
            id,
            mr_human_number,
            &old_statuses,
            Some(&last_commit_id),
            new_status,
            new_approval_status,
            None,
            );
        let projects = &project_set.projects;
        let project = &projects[&project_id];
        {
            *project.ssh_url.lock().unwrap() = ssh_url.to_owned();
        }

        if needs_notification {
            let job_type = match new_approval_status {
                Some(ApprovalStatus::Approved) => JobType::Merge,
                _ => JobType::Try,
            };
            let new_task = WorkerTask {
                id: id,
                job_type: job_type,
                approval_status: new_approval_status.unwrap_or(ApprovalStatus::Pending),
            };

            let &(ref list, ref cvar) = worker_queue;
            list.lock().unwrap().push_back(new_task);
            cvar.notify_one();
            info!("Notified...");
        }
    }

    save_state(state_save_dir, &project_set.name, mr_storage);
    debug!("handle_comment finished      : {}", time::precise_time_ns());
    Ok(Response::with(status::Ok))
}

fn perform_or_continue_jenkins_build(
    mr_id: &MrUid,
    mr_storage: &Mutex<HashMap<MrUid, MergeRequest>>,
    project_set: &ProjectSet,
    config: &toml::Value,
    run_type: &str,
    state_save_dir: &str)
    -> (String, String)
{
    use SubStatusBuilding::*;
    let mut current_build_status: SubStatusBuilding;

    let projects = &project_set.projects;
    let project = &projects[&mr_id.target_project_id];
    let workspace_dir = &project.workspace_dir.to_str().unwrap();
    let http_user = config.lookup("jenkins.user").unwrap().as_str().unwrap();
    let http_password = config.lookup("jenkins.password").unwrap().as_str().unwrap();
    let jenkins_job_url = &project.job_url;

    let current_status;
    {
        let mrs = mr_storage.lock().unwrap();
        let mr = mrs.get(&mr_id).unwrap();
        debug!("MR to work on: {:?}", mr);
        current_status = mr.status.clone();
    };
    if let Status::Open(sso) = current_status {
        if let SubStatusOpen::Building(ssb) = sso {
            current_build_status = ssb;
        } else {
            panic!("MR {:?} is open, but not building", mr_id);
        }
    } else {
        panic!("MR {:?} is not open", mr_id);
    }

    if let NotStarted = current_build_status {
        let queue_item_url = jenkins::enqueue_build(workspace_dir, http_user, http_password, jenkins_job_url, run_type);
        info!("Queue item URL: {}", queue_item_url);
        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            r.status =
                Status::Open(SubStatusOpen::Building(SubStatusBuilding::Queued(
                    queue_item_url.clone())));
        }
        current_build_status = Queued(queue_item_url.clone());
        save_state(state_save_dir, &project_set.name, mr_storage);
    }

    if let Queued(queue_item_url) = current_build_status {
        let build_url = jenkins::poll_queue(workspace_dir, http_user, http_password, &queue_item_url);
        info!("Build job URL: {}", queue_item_url);
        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            r.status =
                Status::Open(SubStatusOpen::Building(SubStatusBuilding::InProgress(
                    build_url.clone())));
        }
        current_build_status = InProgress(build_url.clone());
        save_state(state_save_dir, &project_set.name, mr_storage);
    }
    if let InProgress(build_url) = current_build_status {
        let result = jenkins::poll_build(workspace_dir, http_user, http_password, &build_url);
        info!("Result: {}", result);
        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            r.status =
                Status::Open(SubStatusOpen::Building(SubStatusBuilding::Finished(
                    build_url.clone(), result.clone())));
        }
        current_build_status = Finished(build_url.clone(), result.clone());
        save_state(state_save_dir, &project_set.name, mr_storage);
    }
    if let Finished(build_url, result) = current_build_status {
        (build_url, result)
    } else {
        unreachable!();
    }
}

fn handle_build_request(
    mr_storage: &Mutex<HashMap<MrUid, MergeRequest>>,
    queue: &(Mutex<LinkedList<WorkerTask>>, Condvar),
    config: &toml::Value,
    project_set: &ProjectSet,
    state_save_dir: &str,
    gitlab_session: &gitlab::Session) -> !
{
    debug!("handle_build_request started : {}", time::precise_time_ns());
    let key_path = config.lookup("gitlab.ssh-key-path").unwrap().as_str().unwrap();

    loop {
        debug!("handle_build_request iterated: {}", time::precise_time_ns());

        let &(ref mutex, ref cvar) = queue;
        info!("Waiting to get the request...");
        let mut list = mutex.lock().unwrap();
        while list.is_empty() {
            list = cvar.wait(list).unwrap();
        }
        info!("{:?}", &*list);

        let request = list.pop_front().unwrap();
        info!("Got the request: {:?}", request);

        let mr_id = request.id;
        let mr_human_number;
        let arg;
        {
            let mr_storage = mr_storage.lock().unwrap();
            let ref mr = mr_storage[&mr_id];
            mr_human_number = mr.human_number;
            arg = mr.checkout_sha.to_owned();
        }
        let projects = &project_set.projects;
        let project = &projects[&mr_id.target_project_id];
        let ssh_url = &project.ssh_url;
        let workspace_dir = &project.workspace_dir.to_str().unwrap();

        let mut do_update = false;
        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            if let Status::Open(SubStatusOpen::Updating(SubStatusUpdating::Finished)) = r.status {
                ;
            } else {
                if let Status::Open(SubStatusOpen::Updating(_)) = r.status {
                    r.status =
                        Status::Open(SubStatusOpen::Updating(SubStatusUpdating::InProgress));
                    do_update = true;
                }
            }
        }
        save_state(state_save_dir, &project_set.name, mr_storage);

        if do_update {
            let message = &*format!("{{ \"note\": \":hourglass: проверяю коммит {}\"}}", arg);
            gitlab_session.post_comment(mr_id, message);

            for p in project_set.projects.values() {
                info!("Resetting project {}", p.name);
                let workspace_dir = p.workspace_dir.to_str().unwrap();
                git::set_remote_url(workspace_dir, &p.ssh_url.lock().unwrap());
                git::set_user(workspace_dir, "Shurik", "shurik@example.com");
                git::fetch(workspace_dir, key_path);
                git::reset_hard(workspace_dir, None);

                git::checkout(workspace_dir, "try");
                git::reset_hard(workspace_dir, Some("origin/master"));
                git::push(workspace_dir, key_path, true);
            }

            git::set_remote_url(workspace_dir, &ssh_url.lock().unwrap());
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
                    gitlab_session.post_comment(mr_id, message);
                    continue;
                }
            }
            git::push(workspace_dir, key_path, true);
        }
        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            if let Status::Open(SubStatusOpen::Updating(SubStatusUpdating::InProgress)) = r.status {
                r.status =
                    Status::Open(SubStatusOpen::Updating(SubStatusUpdating::Finished));
            }
        }
        save_state(state_save_dir, &project_set.name, mr_storage);

        let http_user = config.lookup("jenkins.user").unwrap().as_str().unwrap();
        let http_password = config.lookup("jenkins.password").unwrap().as_str().unwrap();
        let jenkins_job_url = &project.job_url;
        debug!("{} {} {}", http_user, http_password, jenkins_job_url);

        let run_type = if request.job_type == JobType::Merge {
            "deploy"
        } else {
            "try"
        };
        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            if let Status::Open(SubStatusOpen::Updating(SubStatusUpdating::Finished)) = r.status {
                r.status =
                    Status::Open(SubStatusOpen::Building(SubStatusBuilding::NotStarted));
            }
        }
        save_state(state_save_dir, &project_set.name, mr_storage);

        let mut do_build = false;
        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            if let Status::Open(SubStatusOpen::WaitingForCi) = r.status {
                r.status =
                    Status::Open(SubStatusOpen::Building(SubStatusBuilding::NotStarted));
            }
            if let Status::Open(SubStatusOpen::Building(SubStatusBuilding::Finished(_, _))) = r.status {
                ;
            } else {
                if let Status::Open(SubStatusOpen::Building(_)) = r.status {
                    do_build = true;
                }
            }
        }
        save_state(state_save_dir, &project_set.name, mr_storage);

        let (build_url, result_string);
        if do_build {
            let result = perform_or_continue_jenkins_build(
                &mr_id,
                mr_storage,
                project_set,
                config,
                run_type,
                state_save_dir);
            build_url = result.0;
            result_string = result.1;
        } else {
            let mrs = mr_storage.lock().unwrap();
            let r = mrs.get(&mr_id).unwrap();
            match r.status {
                Status::Open(SubStatusOpen::Building(SubStatusBuilding::Finished(ref bu, ref rs))) => {
                    build_url = bu.clone();
                    result_string = rs.clone();
                },
                Status::Open(SubStatusOpen::WaitingForResultDispatch(ref bu, ref rs)) => {
                    info!("MR is already waiting for result dispatch");
                    if let Some(ref bu) = *bu {
                        build_url = bu.clone();
                    } else {
                        panic!("Expected status to have build url, but it's {:?}'", bu);
                    }
                    if let Some(ref rs) = *rs {
                        result_string = rs.clone();
                    } else {
                        panic!("Expected status to have result string, but it's {:?}'", rs);
                    }
                },
                _ => panic!("Expected finished build or waiting for result dispatch, but status is {:?}", r.status),
            }
        }

        {
            let mut mrs = mr_storage.lock().unwrap();
            let mut r = mrs.get_mut(&mr_id).unwrap();
            let current_status = r.status.clone();
            if let Status::Open(SubStatusOpen::Building(SubStatusBuilding::Finished(build_url, result_string))) = current_status
            {
                r.status =
                    Status::Open(SubStatusOpen::WaitingForResultDispatch(Some(build_url), Some(result_string)));
            }
        }
        save_state(state_save_dir, &project_set.name, mr_storage);

        {
            let mr_storage = mr_storage.lock().unwrap();
            if let Some(new_request) = mr_storage.get(&mr_id) {
                let ref new_checkout_sha = mr_storage[&mr_id].checkout_sha;
                let checkout_sha = arg;

                if *new_checkout_sha == checkout_sha {
                    if request.approval_status != new_request.approval_status
                        && new_request.approval_status != ApprovalStatus::Approved
                    {
                        info!("The MR was rejected in the meantime, not merging");
                        let message = &*format!("{{ \"note\": \":no_entry_sign: пока мы тестировали, MR запретили. Не сливаю\"}}");
                        gitlab_session.post_comment(mr_id, message);
                        continue;
                    }
                } else {
                    info!("MR head is different commit, not merging");
                    let message = &*format!("{{ \"note\": \":no_entry_sign: пока мы тестировали, MR обновился. Не сливаю\"}}");
                    gitlab_session.post_comment(mr_id, message);
                    continue;
                }
            } else {
                info!("MR was closed or merged in the meantime, skipping");
                continue;
            }
        }
        let mut do_merge = false;
        {
            let mut mr_storage_locked = mr_storage.lock().unwrap();
            let request = mr_storage_locked.get_mut(&mr_id).unwrap();
            let result_string;
            match request.status {
                Status::Open(SubStatusOpen::WaitingForResultDispatch(_, Some(ref rs))) => {
                    result_string = rs;
                },
                ref r => panic!("Expected status to be waiting for result dispatch, but got {:?}", r),
            }
            info!("Result: {}, approval status: {:?}", result_string, request.approval_status);
            if request.approval_status == ApprovalStatus::Approved
                && result_string == "SUCCESS"
            {
                do_merge = true;
            }
        }
        if do_merge {
            info!("Merging");
            let message = &*format!("{{ \"note\": \":sunny: тесты прошли, сливаю\"}}");
            gitlab_session.post_comment(mr_id, message);
            git::checkout(workspace_dir, "master");
            match git::merge(workspace_dir, "try", mr_human_number, true) {
                Ok(_) => {},
                Err(_) => {
                    let message = &*format!("{{ \"note\": \":umbrella: не смог слить MR. Пожалуйста, обновите его (rebase или merge)\"}}");
                    gitlab_session.post_comment(mr_id, message);
                    continue;
                },
            }
            git::status(workspace_dir);
            git::push(workspace_dir, key_path, false);
            {
                let mut mr_storage_locked = mr_storage.lock().unwrap();
                // MR was merged, removing
                mr_storage_locked.remove(&mr_id);
            }
            save_state(state_save_dir, &project_set.name, mr_storage);
            info!("Updated existing MR");
            let message = &*format!("{{ \"note\": \":ok_hand: успешно\"}}");
            gitlab_session.post_comment(mr_id, message);

            for mr in mr_storage.lock().unwrap().values() {
                info!("MR to try merge: {:?}", mr);
                git::set_remote_url(workspace_dir, &ssh_url.lock().unwrap());
                git::set_user(workspace_dir, "Shurik", "shurik@example.com");
                mr_try_merge_and_report_if_impossible(mr, &gitlab_session, workspace_dir, key_path);
            }
            continue;
        } else {
            let build_result_message = match &*result_string {
                "SUCCESS" => "успешно",
                _ => "с ошибками",
            };
            let indicator = match &*result_string {
                "SUCCESS" => ":sunny:",
                _ => ":bangbang:",
            };

            let message = &*format!("{{ \"note\": \"{} тестирование завершено [{}]({})\"}}", indicator, build_result_message, build_url);

            gitlab_session.post_comment(mr_id, message);
            {
                let mut mrs = mr_storage.lock().unwrap();
                let mut r = mrs.get_mut(&mr_id).unwrap();
                if let Status::Open(SubStatusOpen::WaitingForResultDispatch(_, _)) = r.status
                {
                    r.status =
                        Status::Open(SubStatusOpen::WaitingForReview);
                }
            }
            save_state(state_save_dir, &project_set.name, mr_storage);

        }

        debug!("handle_build_request finished: {}", time::precise_time_ns());
    }
}

fn mr_try_merge_and_report_if_impossible(mr: &MergeRequest,
                                         gitlab_session: &gitlab::Session,
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
        let message = &*format!("{{ \"note\": \":umbrella: в результате изменений целевой ветки, этот MR больше нельзя слить. Пожалуйста, обновите его (rebase или merge). Проверенный коммит: {}\"}}", arg);
        gitlab_session.post_comment(mr_id, message);
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
            let message = &*format!("{{ \"note\": \":umbrella: не удалось слить master в MR. Пожалуйста, обновите его (rebase или merge). Проверенный коммит: {}\"}}", arg);
            gitlab_session.post_comment(mr_id, message);
            return;
        }
    }
}

fn scan_state_and_schedule_jobs(
    mr_storage: &Mutex<HashMap<MrUid, MergeRequest>>,
    _project_set: &ProjectSet,
    queue: &(Mutex<LinkedList<WorkerTask>>, Condvar))
{
    let mr_storage = &*mr_storage.lock().unwrap();
    for mr in mr_storage.values() {
        use Status::*;

        let ref status = mr.status;
        match *status {
            Open(ref substatus_open) => {
                use SubStatusOpen::*;

                match *substatus_open {
                    Updating(_) | WaitingForCi | Building(_) | WaitingForResultDispatch(_, _) => {
                        let job_type = match mr.approval_status {
                            ApprovalStatus::Approved => JobType::Merge,
                            _ => JobType::Try,
                        };
                        let new_task = WorkerTask {
                            id: mr.id,
                            job_type: job_type,
                            approval_status: mr.approval_status,
                        };
                        let &(ref list, ref cvar) = queue;
                        list.lock().unwrap().push_back(new_task);
                        cvar.notify_one();
                        info!("Notified...");
                    },
                    _ => {},
                }
            }
            _ => {},
        }
    }
}

fn main() {
    env_logger::init().unwrap();

    let _ =
        App::new("shurik")
        .version("0.1")
        .author("Mikhail Pankov <mikhail.pankov@kaspersky.com>")
        .about("A commit gatekeeper for SDK")
        .get_matches();

    let value = open_parse_config();
    let config: Arc<toml::Value> = Arc::new(value.unwrap());

    let (gitlab_address, gitlab_port) = get_gitlab_listener_config(&*config).unwrap();

    let project_sets = produce_project_sets(&*config).unwrap();

    let mut router = router::Router::new();
    let mut builders = Vec::new();
    let state_save_dir = config.lookup("general.state-save-dir").unwrap().as_str().unwrap();

    let gitlab_login_config = get_gitlab_login_config(&*config).unwrap();
    let gitlab_user = &gitlab_login_config.user;
    let gitlab_password = &gitlab_login_config.password;
    let gitlab_api_root = gitlab_login_config.api_root;

    let gitlab_api = gitlab::Api::new(gitlab_api_root).unwrap();
    let gitlab_session = gitlab_api.login(gitlab_user, gitlab_password).unwrap();
    let gitlab_session = Arc::new(gitlab_session);

    for (psid, project_set) in project_sets.into_iter() {
        let mut reverse_project_map = HashMap::new();
        let gitlab_session = gitlab_session.clone();

        debug!("Handling ProjectSet: {} = {:?}", psid, project_set);

        let psa = Arc::new(project_set);
        let psa2 = psa.clone();
        let psa3 = psa.clone();
        let psa4 = psa.clone();

        let projects = &psa.clone().projects;
        for (id, p) in projects {
            if let Some(ps) = reverse_project_map.insert(id.clone(), psid) {
                panic!("A project with id {}: {:?} is already present in project set with id {}: {:?}. Project can be present only in one project set.", id, p, psid, ps);
            }
        }
        let mr_storage = load_state(state_save_dir, psid);
        let mr_storage = Arc::new(Mutex::new(mr_storage));
        let mrs2 = mr_storage.clone();
        let mrs3 = mrs2.clone();
        let mrs4 = mrs3.clone();

        let queue = Arc::new((Mutex::new(LinkedList::new()), Condvar::new()));
        let queue2 = queue.clone();
        let queue3 = queue.clone();

        let config2 = config.clone();
        let config3 = config2.clone();

        let state_save_dir =
            Arc::new(
                config.lookup("general.state-save-dir").unwrap()
                    .as_str().unwrap()
                    .to_owned());
        let ssd2 = state_save_dir.clone();
        let ssd3 = ssd2.clone();

        let builder = thread::spawn(move || {
            handle_build_request(&*mrs3, &*queue, &*config3, &*psa2, &*ssd3, &gitlab_session);
        });
        builders.push(builder);
        scan_state_and_schedule_jobs(&*mrs4, &*psa4, &*queue2);

        router.post(format!("/api/v1/{}/mr", psid),
                    move |req: &mut Request|
                    handle_mr(req, &*mr_storage, &*psa3, &*state_save_dir));
        router.post(format!("/api/v1/{}/comment", psid),
                    move |req: &mut Request|
                    handle_comment(req, &*mrs2, &*queue3, &*psa, &*ssd2));
    }
    Iron::new(router).http(
        (&*gitlab_address, gitlab_port))
        .expect("Couldn't start the web server");
}

#[derive(Debug)]
pub struct ParseError;

quick_error! {
    #[derive(Debug)]
    pub enum ConfigError {
        Read(err: ::std::io::Error) {
            from()
        }
        Parse(err: ParseError) {
            from()
        }
    }
}

fn open_parse_config() -> Result<toml::Value, ConfigError> {
    let mut file = try!(File::open("Config.toml"));
    let mut toml = String::new();
    try!(file.read_to_string(&mut toml));

    let mut parser = toml::Parser::new(&toml);
    let maybe_value = try!(parser.parse().ok_or(ParseError));
    let value: toml::Value = toml::Value::Table(maybe_value);
    Ok(value)
}

#[derive(Debug)]
pub struct IntegerConversionError;
#[derive(Debug)]
pub struct StrConversionError;
#[derive(Debug)]
pub struct UrlConversionError;
#[derive(Debug)]
pub struct SliceConversionError;

quick_error! {
    #[derive(Debug)]
    pub enum ListenerConfigError {
        IntegerConversion(err: IntegerConversionError) {
            from()
        }
        StrConversion(err: StrConversionError) {
            from()
        }
    }
}

fn get_gitlab_listener_config(
    config: &toml::Value)
    -> Result<(String, u16), ListenerConfigError> {

    let default_port_value = toml::Value::String("10042".to_owned());
    let gitlab_port_value = config
        .lookup("gitlab.webhook-port")
        .unwrap_or(&default_port_value);
    let gitlab_port = try!(
        gitlab_port_value.as_integer().ok_or(IntegerConversionError))
        as u16;

    let default_address_value = toml::Value::String("localhost".to_owned());
    let gitlab_address_value = config
        .lookup("gitlab.webhook-address")
        .unwrap_or(&default_address_value);
    let gitlab_address = try!(
        gitlab_address_value.as_str().ok_or(StrConversionError)).to_owned();
    info!("GitLab port: {}", gitlab_port);
    info!("GitLab address: {}", gitlab_address);
    Ok((gitlab_address, gitlab_port))
}

struct LoginConfig {
    user: String,
    password: String,
    api_root: hyper::Url,
}

#[derive(Debug)]
pub struct UserLookupError;
#[derive(Debug)]
pub struct PasswordLookupError;
#[derive(Debug)]
pub struct ApiRootLookupError;
#[derive(Debug)]
pub struct GenericLookupError;

quick_error! {
    #[derive(Debug)]
    pub enum LoginConfigError {
        UserLookup(err: UserLookupError) {
            from()
        }
        PasswordLookup(err: PasswordLookupError) {
            from()
        }
        ApiRootLookup(err: ApiRootLookupError) {
            from()
        }
        IntegerConversion(err: IntegerConversionError) {
            from()
        }
        StrConversion(err: StrConversionError) {
            from()
        }
        UrlConversion(err: UrlConversionError) {
            from()
        }
    }
}

quick_error! {
    #[derive(Debug)]
    pub enum ProjectSetProductionError {
        GenericLookup(err: GenericLookupError) {
            from()
        }
        IntegerConversion(err: IntegerConversionError) {
            from()
        }
        StrConversion(err: StrConversionError) {
            from()
        }
        SliceConversion(err: SliceConversionError) {
            from()
        }
    }
}

fn get_gitlab_login_config(
    config: &toml::Value)
    -> Result<LoginConfig, LoginConfigError> {

    let gitlab_user_value = try!(
        config
            .lookup("gitlab.user")
            .ok_or(UserLookupError));
    let gitlab_user = try!(
        gitlab_user_value.as_str().ok_or(StrConversionError));

    let gitlab_password_value = try!(
        config
            .lookup("gitlab.password")
            .ok_or(PasswordLookupError));
    let gitlab_password = try!(
        gitlab_password_value.as_str().ok_or(StrConversionError));

    let gitlab_api_root_value = try!(
        config
            .lookup("gitlab.api-root")
            .ok_or(ApiRootLookupError));
    let gitlab_api_root_string = try!(
        gitlab_api_root_value.as_str().ok_or(StrConversionError));
    let gitlab_api_root = try!(
        gitlab_api_root_string.parse().map_err(|_| UrlConversionError));

    Ok(LoginConfig {
        user: gitlab_user.to_owned(),
        password: gitlab_password.to_owned(),
        api_root: gitlab_api_root,
    })
}

macro_rules! config_lookup_convert {
    ($config:ident :
     $path:expr => $lookup_error:expr,
     $conversion_fn:ident => $conversion_error:expr) => {
        {
            let value = try!(
                $config.lookup($path).ok_or($lookup_error));
            let result = try!(
                value.$conversion_fn().ok_or($conversion_error));
            result
        }
    }
}

fn produce_project_sets(
    config: &toml::Value)
    -> Result<HashMap<&str, ProjectSet>, ProjectSetProductionError> {

    let mut project_sets = HashMap::new();

    let project_set_slice = config_lookup_convert!(
        config :
        "project-set" => GenericLookupError,
        as_slice => SliceConversionError);

    for project_set_toml in project_set_slice {
        let mut projects = HashMap::new();
        let name = config_lookup_convert!(
            project_set_toml :
            "name" => GenericLookupError,
            as_str => StrConversionError);

        let project_slice = config_lookup_convert!(
            project_set_toml :
            "project" => GenericLookupError,
            as_slice => SliceConversionError);

        for project_toml in project_slice {
            let key = config_lookup_convert!(
                project_toml :
                "id" => GenericLookupError,
                as_integer => IntegerConversionError)
                as u64;
            let reviewers_slice = config_lookup_convert!(
                project_toml :
                "reviewers" => GenericLookupError,
                as_slice => SliceConversionError);
            if reviewers_slice.len() == 0 {
                panic!("Project has no reviewers! That would make it impossible to maintain it. Project in question: {:?}", project_toml);
            }
            let str_vec: Vec<&str> =
                reviewers_slice
                .iter()
                .map(
                    |x| x.as_str().ok_or(StrConversionError))
                .take_while(
                    |x| x.is_ok())
                .map(
                    |x| x.unwrap())
                .collect();
            if str_vec.len() != reviewers_slice.len() {
                return Err(From::from(StrConversionError));
            }

            let string_vec: Vec<String> = str_vec.iter().map(|x: &&str| -> String { (*x).to_owned() }).collect();

            let job_url = config_lookup_convert!(
                project_toml :
                "job-url" => GenericLookupError,
                as_str => StrConversionError);
            let name = config_lookup_convert!(
                project_toml :
                "name" => GenericLookupError,
                as_str => StrConversionError);
            let ssh_url = config_lookup_convert!(
                project_toml :
                "ssh-url" => GenericLookupError,
                as_str => StrConversionError);
            let workspace_dir = PathBuf::from(
                config_lookup_convert!(
                    project_toml :
                    "workspace-dir" => GenericLookupError,
                    as_str => StrConversionError));

            let p = Project {
                id: key,
                workspace_dir: workspace_dir,
                reviewers: string_vec,
                job_url: job_url.to_owned(),
                name: name.to_owned(),
                ssh_url: Mutex::new(ssh_url.to_owned())
            };
            projects.insert(key, p);
        }
        info!("Read projects: {:?}", projects);

        let new_ps = ProjectSet { name: name.to_owned(), projects: projects };
        if let Some(ps) = project_sets.insert(name, new_ps) {
            panic!("Project set with name {} is already defined: {:?}. Attempted to define another project set with such name: {:?}. The name must be unique.", name, ps, project_sets[name]);
        }
    }

    Ok(project_sets)
}
