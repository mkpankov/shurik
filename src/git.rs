use ::std::process::{Command, ExitStatus};

pub fn fetch(workspace_dir: &str, key_path: &str) {
    let git_fetch_command = format!("ssh-add {} && git fetch", key_path);
    let status = Command::new("ssh-agent")
        .arg("sh").arg("-c").arg(git_fetch_command)
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Fetched remote");
    } else {
        panic!("Couldn't fetch remote: {}", status)
    }
}

pub fn checkout(workspace_dir: &str, branch: &str) {
    let status = Command::new("git")
        .arg("checkout").arg(branch)
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Checked out {}", branch);
    } else {
        panic!("Couldn't checkout the {} branch: {}", branch, status)
    }
}

pub fn reset_hard(workspace_dir: &str, to: Option<&str>) {
    let mut command = Command::new("git");
    let mut builder = command
        .arg("reset").arg("--hard").current_dir(workspace_dir);
    if let Some(to) = to {
        builder.arg(to);
    }
    let status = builder.status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Reset current branch to {}", to.unwrap_or("HEAD"));
    } else {
        panic!("Couldn't reset current branch: {}", status)
    }
}

pub fn status(workspace_dir: &str) {
    let status = Command::new("git")
        .arg("status")
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Queried status");
    } else {
        panic!("Couldn't query the git status: {}", status)
    }
}

pub fn push(workspace_dir: &str, key_path: &str, do_force: bool) {
    let git_push_command = if do_force {
        format!("ssh-add {} && git push -u --force", key_path)
    } else {
        format!("ssh-add {} && git push -u", key_path)
    };

    let status = Command::new("ssh-agent")
        .arg("sh").arg("-c").arg(git_push_command)
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Push current branch");
    } else {
        panic!("Couldn't push the current branch: {}", status)
    }
}

pub fn merge(workspace_dir: &str, branch: &str, mr_human_number: u64, no_ff: bool) -> Result<(), String> {
    let mut command = Command::new("git");
    let mut builder = command
        .arg("merge").arg(branch)
        .current_dir(workspace_dir);
    if no_ff {
        builder.arg("--no-ff");
        builder.arg(&*format!("-m \"Merging MR !{}\"", mr_human_number));
    } else {
        builder.arg(&*format!("-m \"Updating MR !{}\"", mr_human_number));
    }
    let status = builder
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Merge {}", branch);
        Ok(())
    } else {
        Err(format!("Couldn't merge the {} branch: {}", branch, status))
    }
}

#[allow(unused)]
pub fn rebase(workspace_dir: &str, to: &str) -> Result<(), String> {
    let status = Command::new("git")
        .arg("rebase").arg(to)
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Rebase MR to 'master'");
        Ok(())
    } else {
        Err(format!("Couldn't rebase MR to 'master': {}", status))
    }
}

pub fn set_remote_url(workspace_dir: &str, url: &str) {
    let status = Command::new("git")
        .arg("remote").arg("set-url").arg("origin").arg(url)
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Set remote 'origin' URL to {}", url);
    } else {
        panic!("Couldn't set remote 'origin' URL: {}", status)
    }
}

pub fn set_user(workspace_dir: &str, name: &str, email: &str) {
    let status = Command::new("git")
        .arg("config").arg("user.name").arg(name)
        .current_dir(workspace_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        info!("Set user name to {}", name);
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
        info!("Set user email to {}", email);
    } else {
        panic!("Couldn't set user email: {}", status)
    }
}
