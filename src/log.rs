use std::process::Command;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogResult {
    pub branch: Option<String>,
    pub head: String,
    pub commits: Vec<LogCommit>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogCommit {
    pub id: String,
    pub parents: Vec<String>,
    pub subject: String,
}

pub fn inspect() -> Result<LogResult, String> {
    let head = git_output(&["rev-parse", "HEAD"])?;
    let branch = git_optional(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let output = git_output(&["log", "--format=%H%x09%P%x09%s"])?;
    let commits = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.splitn(3, "\t");
            let id = fields
                .next()
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            let parents = fields
                .next()
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            let subject = fields
                .next()
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            Ok(LogCommit {
                id: id.to_owned(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                subject: subject.to_owned(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(LogResult {
        branch: branch.map(|value| value.trim().to_owned()),
        head: head.trim().to_owned(),
        commits,
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
