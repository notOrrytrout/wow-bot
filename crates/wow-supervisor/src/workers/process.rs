use anyhow::Result;
use std::process::Stdio;
use tokio::process::{Child, Command};
pub async fn spawn_worker(program: &str, args: &[String]) -> Result<Child> {
    Ok(Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?)
}
