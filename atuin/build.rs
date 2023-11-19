use std::process::Command;
fn main() {
    let output: Result<std::process::Output, std::io::Error> = Command::new("git").args(["rev-parse", "HEAD"]).output();

    let sha: String = match output {
        Ok(sha) => String::from_utf8(sha.stdout).unwrap(),
        Err(_) => String::from("NO_GIT"),
    };

    println!("cargo:rustc-env=GIT_HASH={}", sha);
}
