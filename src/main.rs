extern crate clap;
use clap::App;

fn main() {
    let matches =
        App::new("shurik")
        .version("0.1")
        .author("Mikhail Pankov <mikhail.pankov@kaspersky.com>")
        .about("A commit gatekeeper for SDK")
        .args_from_usage(
            "-n --dry-run 'Don\'t actually do anything, just print what is to be done'
             -g --gitlab-port=[GITLAB_PORT] 'Port to listen for GitLab WebHooks'")
        .get_matches();

    println!("Dry run: {}", matches.is_present("dry-run"));
    println!("GitLab port: {}", matches.value_of("GITLAB_PORT").unwrap_or("10042"));
}
