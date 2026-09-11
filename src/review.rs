use std::{collections::BTreeMap, process::Command};

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewResult {
    pub branch: Option<String>,
    pub base: String,
    pub merge_base: String,
    pub head: String,
    pub commits: Vec<ReviewCommit>,
    pub files: Vec<ReviewFile>,
    pub stat: String,
    pub patch: String,
}

#[derive(Serialize)]
pub struct ReviewCommit {
    pub id: String,
    pub subject: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub status: String,
    pub path: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub hunks: Vec<ReviewHunk>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewHunk {
    pub header: String,
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<String>,
}

pub fn inspect(base: &str) -> Result<ReviewResult, String> {
    let base_commit = git_output(&["rev-parse", "--verify", &format!("{base}^{{commit}}")])?;
    let base_commit = base_commit.trim().to_owned();
    let head = git_output(&["rev-parse", "HEAD"])?;
    let head = head.trim().to_owned();
    let merge_base = git_output(&["merge-base", &base_commit, &head])?;
    let merge_base = merge_base.trim().to_owned();
    let branch = git_optional(&["symbolic-ref", "--quiet", "--short", "HEAD"])?
        .map(|value| value.trim().to_owned());
    let range = format!("{merge_base}..{head}");

    Ok(ReviewResult {
        branch,
        base: base_commit,
        merge_base: merge_base.clone(),
        head: head.clone(),
        commits: commits(&merge_base, &head)?,
        files: files(&range)?,
        stat: git_output(&["diff", "--stat", "--no-renames", &range])?,
        patch: git_output(&[
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--no-renames",
            &range,
        ])?,
    })
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
                .split_once("\t")
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            Ok(ReviewCommit {
                id: id.to_owned(),
                subject: subject.to_owned(),
            })
        })
        .collect()
}

fn files(range: &str) -> Result<Vec<ReviewFile>, String> {
    let statuses = git_output(&["diff", "--name-status", "--no-renames", range])?;
    let numstat = git_output(&["diff", "--numstat", "--no-renames", range])?;
    let mut counts = BTreeMap::new();

    for line in numstat.lines().filter(|line| !line.is_empty()) {
        let mut fields = line.splitn(3, "\t");
        let additions = fields
            .next()
            .ok_or_else(|| "unexpected git numstat output".to_owned())?;
        let deletions = fields
            .next()
            .ok_or_else(|| "unexpected git numstat output".to_owned())?;
        let path = fields
            .next()
            .ok_or_else(|| "unexpected git numstat output".to_owned())?;
        counts.insert(
            path.to_owned(),
            (additions.parse::<u64>().ok(), deletions.parse::<u64>().ok()),
        );
    }

    statuses
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (status, path) = line
                .split_once("\t")
                .ok_or_else(|| "unexpected git diff output".to_owned())?;
            let (additions, deletions) = counts.get(path).copied().unwrap_or((None, None));
            let patch = git_output(&[
                "diff",
                "--unified=3",
                "--no-ext-diff",
                "--no-color",
                "--no-renames",
                range,
                "--",
                path,
            ])?;
            Ok(ReviewFile {
                status: status.to_owned(),
                path: path.to_owned(),
                additions,
                deletions,
                hunks: parse_hunks(&patch),
            })
        })
        .collect()
}

fn parse_hunks(patch: &str) -> Vec<ReviewHunk> {
    let mut hunks = Vec::new();
    let mut current: Option<ReviewHunk> = None;

    for line in patch.lines() {
        if let Some((old_start, old_lines, new_start, new_lines)) = parse_hunk_header(line) {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current = Some(ReviewHunk {
                header: line.to_owned(),
                old_start,
                old_lines,
                new_start,
                new_lines,
                lines: Vec::new(),
            });
        } else if let Some(hunk) = current.as_mut() {
            hunk.lines.push(line.to_owned());
        }
    }

    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    hunks
}

fn parse_hunk_header(line: &str) -> Option<(u64, u64, u64, u64)> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let mut ranges = rest[..end].split_whitespace();
    let old = ranges.next()?.strip_prefix("-")?;
    let new = ranges.next()?.strip_prefix("+")?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;
    Some((old_start, old_lines, new_start, new_lines))
}

fn parse_range(value: &str) -> Option<(u64, u64)> {
    match value.split_once(",") {
        Some((start, lines)) => Some((start.parse().ok()?, lines.parse().ok()?)),
        None => Some((value.parse().ok()?, 1)),
    }
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
