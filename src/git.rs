use ::std::process::{Command, ExitStatus};

pub fn fetch() {
    let status = Command::new("git")
        .arg("fetch")
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

pub fn reset_hard(to: &str) {
    let status = Command::new("git")
        .arg("reset").arg("--hard").arg(to)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Reset 'try' to {}", to);
    } else {
        panic!("Couldn't reset the 'try' branch: {}", status)
    }
}

pub fn push(do_force: bool) {
    let mut command = Command::new("git");
    let builder = command
        .arg("push");

    if do_force {
        builder.arg("--force-with-lease");
    }

    let builder = builder
        .current_dir("workspace/shurik");

    let status = builder
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

pub fn merge_ff(mr_human_number: u64) {
    let status = Command::new("git")
        .arg("merge").arg("try").arg("--ff-only")
        .arg(&*format!("-m \"Merging MR #{}\"", mr_human_number))
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Merge master to MR {}", mr_human_number);
    } else {
        panic!("Couldn't merge the 'master' branch: {}", status)
    }
}

pub fn rebase(to: &str) {
    let status = Command::new("git")
        .arg("rebase").arg(to)
        .current_dir("workspace/shurik")
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to execute process: {}", e)
        });
    if ExitStatus::success(&status) {
        println!("Rebase MR to 'master'");
    } else {
        panic!("Couldn't rebase MR to 'master': {}", status)
    }
}
