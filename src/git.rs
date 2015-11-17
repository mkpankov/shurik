use ::std::process::{Command, ExitStatus};

pub fn fetch() {
    let git_fetch_command = "ssh-add /home/mkpankov/.ssh/shurik-host.id_rsa && git fetch";
    let status = Command::new("ssh-agent")
        .arg("sh").arg("-c").arg(git_fetch_command)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Fetched remote");
    } else {
        panic!("Couldn't fetch remote: {}", status)
    }
}

pub fn checkout(branch: &str) {
    let status = Command::new("git")
        .arg("checkout").arg(branch)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Checked out {}", branch);
    } else {
        panic!("Couldn't checkout the {} branch: {}", branch, status)
    }
}

pub fn reset_hard(to: Option<&str>) {
    let mut command = Command::new("git");
    let mut builder = command
        .arg("reset").arg("--hard").current_dir("workspace/shurik");
    if let Some(to) = to {
        builder.arg(to);
    }
    let status = builder.status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Reset current branch to {}", to.unwrap_or("HEAD"));
    } else {
        panic!("Couldn't reset current branch: {}", status)
    }
}

pub fn status() {
    let status = Command::new("git")
        .arg("status")
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Queried status");
    } else {
        panic!("Couldn't query the git status: {}", status)
    }
}

pub fn push(do_force: bool) {
    let git_push_command = if do_force {
        "ssh-add /home/mkpankov/.ssh/shurik-host.id_rsa && git push -u --force-with-lease"
    } else {
        "ssh-add /home/mkpankov/.ssh/shurik-host.id_rsa && git push -u"
    };

    let status = Command::new("ssh-agent")
        .arg("sh").arg("-c").arg(git_push_command)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Push current branch");
    } else {
        panic!("Couldn't push the current branch: {}", status)
    }
}

pub fn merge(branch: &str, mr_human_number: u64) -> Result<(), String> {
    let status = Command::new("git")
        .arg("merge").arg(branch).arg("--no-ff")
        .arg(&*format!("-m \"Merging MR !{}\"", mr_human_number))
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Merge master to MR {}", mr_human_number);
        Ok(())
    } else {
        Err(format!("Couldn't merge the 'master' branch: {}", status))
    }
}

#[allow(unused)]
pub fn rebase(to: &str) -> Result<(), String> {
    let status = Command::new("git")
        .arg("rebase").arg(to)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Rebase MR to 'master'");
        Ok(())
    } else {
        Err(format!("Couldn't rebase MR to 'master': {}", status))
    }
}

pub fn set_remote_url(url: &str) {
    let status = Command::new("git")
        .arg("remote").arg("set-url").arg("origin").arg(url)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Set remote 'origin' URL to {}", url);
    } else {
        panic!("Couldn't set remote 'origin' URL: {}", status)
    }
}

pub fn set_user(name: &str, email: &str) {
    let status = Command::new("git")
        .arg("config").arg("user.name").arg(name)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Set user name to {}", name);
    } else {
        panic!("Couldn't set user name: {}", status)
    }
    let status = Command::new("git")
        .arg("config").arg("user.email").arg(email)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Set user email to {}", email);
    } else {
        panic!("Couldn't set user email: {}", status)
    }
}
