use std::process::Command;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewResult {
    pub base: String,
    pub merge_base: String,
    pub head: String,
    pub commits: Vec<ReviewCommit>,
    pub files: Vec<ReviewFile>,
}

#[derive(Serialize)]
pub struct ReviewCommit {
    pub id: String,
    pub subject: String,
}

#[derive(Serialize)]
pub struct ReviewFile {
    pub status: String,
    pub path: String,
}

pub fn inspect(base: &str) -> Result<ReviewResult, String> {
    let base_commit = git_output(&["rev-parse", "--verify", &format!("{base}^{{commit}}")])?;
    let base_commit = base_commit.trim().to_owned();
    let head = git_output(&["rev-parse", "HEAD"])?;
    let head = head.trim().to_owned();
    let merge_base = git_output(&["merge-base", &base_commit, &head])?;
    let merge_base = merge_base.trim().to_owned();

    Ok(ReviewResult {
        base: base_commit,
        merge_base: merge_base.clone(),
        head: head.clone(),
        commits: commits(&merge_base, &head)?,
        files: files(&merge_base, &head)?,
    })
}

pub fn show_patch(base: &str) -> Result<(), String> {
    run_git(&["diff", &format!("{base}...HEAD")], "review diff")
}

pub fn show_stat(base: &str) -> Result<(), String> {
    run_git(
        &["diff", "--stat", &format!("{base}...HEAD")],
        "review stat",
    )
}

pub fn show_files(base: &str) -> Result<(), String> {
    run_git(
        &[
            "diff",
            "--name-status",
            "--no-renames",
            &format!("{base}...HEAD"),
        ],
        "review files",
    )
}

pub fn show_commits(base: &str) -> Result<(), String> {
    let merge_base = git_output(&["merge-base", base, "HEAD"])?;
    run_git(
        &["log", "--oneline", &format!("{}..HEAD", merge_base.trim())],
        "review commits",
    )
}

fn commits(merge_base: &str, head: &str) -> Result<Vec<ReviewCommit>, String> {
    let output = git_output(&[
        "log",
        "--reverse",
        "--format=%H%x09%s",
        &format!("{merge_base}..{head}"),
    ])?;

    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (id, subject) = line
                .split_once('\t')
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            Ok(ReviewCommit {
                id: id.to_owned(),
                subject: subject.to_owned(),
            })
        })
        .collect()
}

fn files(merge_base: &str, head: &str) -> Result<Vec<ReviewFile>, String> {
    let output = git_output(&[
        "diff",
        "--name-status",
        "--no-renames",
        &format!("{merge_base}..{head}"),
    ])?;

    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (status, path) = line
                .split_once('\t')
                .ok_or_else(|| "unexpected git diff output".to_owned())?;
            Ok(ReviewFile {
                status: status.to_owned(),
                path: path.to_owned(),
            })
        })
        .collect()
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

fn run_git(args: &[&str], operation: &str) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .status()
        .map_err(|error| format!("failed to run {operation}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{operation} failed"))
    }
}
