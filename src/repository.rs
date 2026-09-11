use std::process::Command;

use serde::Serialize;

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryState {
    pub root: String,
    pub branch: Option<String>,
    pub head: String,
    pub dirty: bool,
    pub worktree: Vec<String>,
}

pub fn inspect() -> Result<RepositoryState, String> {
    let root = git_output(&["rev-parse", "--show-toplevel"])?;
    let head = git_output(&["rev-parse", "HEAD"])?;
    let branch = git_optional(&["symbolic-ref", "--quiet", "--short", "HEAD"])?
        .map(|value| value.trim().to_owned());
    let status = git_output(&["status", "--porcelain=v1"])?;
    let worktree = status.lines().map(str::to_owned).collect::<Vec<_>>();

    Ok(RepositoryState {
        root: root.trim().to_owned(),
        branch,
        head: head.trim().to_owned(),
        dirty: !worktree.is_empty(),
        worktree,
    })
}

fn git_optional(args: &[&str]) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout)
            .map(Some)
            .map_err(|_| "git returned non-UTF-8 output".to_owned())
    } else {
        Ok(None)
    }
}

fn git_output(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout).map_err(|_| "git returned non-UTF-8 output".to_owned())
}
