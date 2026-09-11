use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

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

pub fn state_token() -> Result<String, String> {
    let root = PathBuf::from(git_output(&["rev-parse", "--show-toplevel"])?.trim());
    let mut state = Vec::new();
    state.extend_from_slice(b"HEAD\0");
    state.extend_from_slice(git_bytes(&["rev-parse", "HEAD"])?.as_slice());
    state.extend_from_slice(b"INDEX\0");
    state.extend_from_slice(git_bytes(&["ls-files", "--stage", "-z"])?.as_slice());
    state.extend_from_slice(b"WORKTREE\0");
    state.extend_from_slice(
        git_bytes(&["diff", "--binary", "--no-ext-diff", "--no-color"])?.as_slice(),
    );
    state.extend_from_slice(b"UNTRACKED\0");
    let untracked = git_bytes(&["ls-files", "--others", "--exclude-standard", "-z"])?;
    for raw in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = String::from_utf8(raw.to_vec())
            .map_err(|_| "gut currently requires UTF-8 repository paths".to_owned())?;
        state.extend_from_slice(path.as_bytes());
        state.push(0);
        let contents = fs::read(root.join(&path))
            .map_err(|error| format!("failed to read untracked {path}: {error}"))?;
        state.extend_from_slice(hash_bytes(&contents)?.as_bytes());
        state.push(0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join(&path))
                .map_err(|error| format!("failed to inspect untracked {path}: {error}"))?
                .permissions()
                .mode();
            state.extend_from_slice(format!("{mode:o}").as_bytes());
            state.push(0);
        }
    }
    hash_bytes(&state)
}

fn hash_bytes(bytes: &[u8]) -> Result<String, String> {
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run git hash-object: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "failed to open git hash-object stdin".to_owned())?
        .write_all(bytes)
        .map_err(|error| format!("failed to hash repository state: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for git hash-object: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| "git hash-object returned non-UTF-8 output".to_owned())
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
    String::from_utf8(git_bytes(args)?).map_err(|_| "git returned non-UTF-8 output".to_owned())
}

fn git_bytes(args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}
