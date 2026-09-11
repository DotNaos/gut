use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Update,
    Before,
    After,
    Undo,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::Before => "before",
            Self::After => "after",
            Self::Undo => "undo",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub id: String,
    pub parent: Option<String>,
    pub kind: OperationKind,
    pub branch: String,
    pub target: Option<String>,
    pub head_before: String,
    pub head_after: String,
    pub before_ref: String,
    pub after_ref: String,
    pub created_unix_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationDiff {
    pub operation: OperationRecord,
    pub patch: String,
}

pub struct PendingOperation {
    id: String,
    parent: Option<String>,
    kind: OperationKind,
    branch: String,
    target: Option<String>,
    head_before: String,
    before_ref: String,
    after_ref: String,
    created_unix_ms: u64,
}

pub fn begin(kind: OperationKind, target: Option<String>) -> Result<PendingOperation, String> {
    let branch = current_branch()?;
    let head_before = git_output(&["rev-parse", "HEAD"])?;
    let head_before = head_before.trim().to_owned();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?;
    let id = format!(
        "op-{}-{}-{}",
        now.as_nanos(),
        std::process::id(),
        &head_before[..head_before.len().min(12)]
    );
    let before_ref = format!("refs/gut/operations/{id}/before");
    let after_ref = format!("refs/gut/operations/{id}/after");
    let parent = read_records()?
        .into_iter()
        .rev()
        .find(|record| record.branch == branch)
        .map(|record| record.id);

    git_quiet(&["update-ref", &before_ref, &head_before])?;

    Ok(PendingOperation {
        id,
        parent,
        kind,
        branch,
        target,
        head_before,
        before_ref,
        after_ref,
        created_unix_ms: now.as_millis() as u64,
    })
}

impl PendingOperation {
    pub fn recovery_ref(&self) -> &str {
        &self.before_ref
    }

    pub fn finish(self) -> Result<OperationRecord, String> {
        let head_after = git_output(&["rev-parse", "HEAD"])?;
        let head_after = head_after.trim().to_owned();
        git_quiet(&["update-ref", &self.after_ref, &head_after])?;

        let record = OperationRecord {
            id: self.id,
            parent: self.parent,
            kind: self.kind,
            branch: self.branch,
            target: self.target,
            head_before: self.head_before,
            head_after,
            before_ref: self.before_ref,
            after_ref: self.after_ref,
            created_unix_ms: self.created_unix_ms,
        };
        append_record(&record)?;
        Ok(record)
    }

    pub fn abort(self) {
        let _ = git_quiet(&["update-ref", "-d", &self.before_ref]);
    }
}

pub fn log() -> Result<Vec<OperationRecord>, String> {
    let mut records = read_records()?;
    records.reverse();
    Ok(records)
}

pub fn diff(id: &str) -> Result<OperationDiff, String> {
    let operation = find(id)?;
    let patch = git_output(&["diff", &operation.before_ref, &operation.after_ref])?;
    Ok(OperationDiff { operation, patch })
}

pub fn show_diff(id: &str) -> Result<(), String> {
    let operation = find(id)?;
    let status = Command::new("git")
        .args(["diff", &operation.before_ref, &operation.after_ref])
        .status()
        .map_err(|error| format!("failed to run operation diff: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("operation diff failed".to_owned())
    }
}

pub fn undo(id: Option<&str>) -> Result<OperationRecord, String> {
    require_clean_worktree()?;
    let branch = current_branch()?;
    let records = read_records()?;
    let target = match id {
        Some(id) => records
            .iter()
            .find(|record| record.id == id)
            .cloned()
            .ok_or_else(|| format!("operation not found: {id}"))?,
        None => records
            .iter()
            .rev()
            .find(|record| record.branch == branch)
            .cloned()
            .ok_or_else(|| format!("no operations recorded for branch {branch}"))?,
    };

    if target.branch != branch {
        return Err(format!(
            "operation {} belongs to branch {}, current branch is {}",
            target.id, target.branch, branch
        ));
    }

    let destination = git_output(&[
        "rev-parse",
        "--verify",
        &format!("{}^{{commit}}", target.before_ref),
    ])?;
    let destination = destination.trim().to_owned();
    let pending = begin(OperationKind::Undo, Some(target.id.clone()))?;

    let reset = Command::new("git")
        .args(["reset", "--hard", &destination])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to restore repository state: {error}"))?;
    if !reset.success() {
        pending.abort();
        return Err("failed to restore repository state".to_owned());
    }

    pending.finish()
}

fn find(id: &str) -> Result<OperationRecord, String> {
    read_records()?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| format!("operation not found: {id}"))
}

fn current_branch() -> Result<String, String> {
    let branch = git_output(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        Err("operation history requires an attached branch".to_owned())
    } else {
        Ok(branch.to_owned())
    }
}

fn require_clean_worktree() -> Result<(), String> {
    let status = git_output(&["status", "--porcelain"])?;
    if status.trim().is_empty() {
        Ok(())
    } else {
        Err("undo requires a clean working tree".to_owned())
    }
}

fn read_records() -> Result<Vec<OperationRecord>, String> {
    let path = operations_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .map_err(|error| format!("invalid operation log entry: {error}"))
        })
        .collect()
}

fn append_record(record: &OperationRecord) -> Result<(), String> {
    let path = operations_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    serde_json::to_writer(&mut file, record)
        .map_err(|error| format!("failed to serialize operation: {error}"))?;
    file.write_all(b"\n")
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn operations_path() -> Result<PathBuf, String> {
    let common_dir = git_output(&["rev-parse", "--git-common-dir"])?;
    let path = PathBuf::from(common_dir.trim());
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|error| format!("failed to inspect current directory: {error}"))?
            .join(path)
    };
    Ok(path.join("gut").join("operations.jsonl"))
}

fn git_quiet(args: &[&str]) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git command failed: git {}", args.join(" ")))
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
