use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
};

use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlacementKind {
    Update,
    Before,
    After,
}

pub enum Placement {
    Update(String),
    Before(String),
    After(String),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitPlacementResult {
    pub operation: PlacementKind,
    pub target_before: String,
    pub head_before: String,
    pub head_after: String,
}

impl Placement {
    pub fn from_args(
        update: Option<String>,
        before: Option<String>,
        after: Option<String>,
    ) -> Result<Self, String> {
        match (update, before, after) {
            (Some(target), None, None) => Ok(Self::Update(target)),
            (None, Some(target), None) => Ok(Self::Before(target)),
            (None, None, Some(target)) => Ok(Self::After(target)),
            _ => Err("choose exactly one of --update, --before, or --after".to_owned()),
        }
    }
}

pub fn place_current_changes(
    placement: Placement,
    message: Option<&str>,
    quiet: bool,
) -> Result<CommitPlacementResult, String> {
    let (target_input, mode, operation) = match &placement {
        Placement::Update(target) => (target.as_str(), "update", PlacementKind::Update),
        Placement::Before(target) => (target.as_str(), "before", PlacementKind::Before),
        Placement::After(target) => (target.as_str(), "after", PlacementKind::After),
    };

    if matches!(placement, Placement::Update(_)) && message.is_some() {
        return Err("--message is only valid with --before or --after".to_owned());
    }

    let target = git_output(&[
        "rev-parse",
        "--verify",
        &format!("{target_input}^{{commit}}"),
    ])?;
    let target = target.trim().to_owned();
    let original_head = git_output(&["rev-parse", "HEAD"])?;
    let original_head = original_head.trim().to_owned();

    require_attached_branch()?;
    require_ancestor(&target, &original_head)?;
    require_linear_history(&target, &original_head)?;

    git(&["add", "-A"])?;
    if git_status(&["diff", "--cached", "--quiet"])? {
        return Err("there are no changes to commit".to_owned());
    }

    match placement {
        Placement::Update(_) => {
            let mut commit = Command::new("git");
            commit.args(["commit", "--fixup", &target]);
            run_git_command(&mut commit, "commit", quiet)?;
            let mut rebase = Command::new("git");
            rebase.args(["rebase", "--interactive", "--autosquash"]);
            add_rebase_base(&mut rebase, &target)?;
            rebase.env("GIT_SEQUENCE_EDITOR", ":");
            run_git_command(&mut rebase, "history rewrite", quiet)?;
        }
        Placement::Before(_) | Placement::After(_) => {
            let mut commit = Command::new("git");
            commit.arg("commit");
            if let Some(message) = message {
                commit.args(["-m", message]);
            }
            run_git_command(&mut commit, "commit", quiet)?;

            let inserted = git_output(&["rev-parse", "HEAD"])?;
            let inserted = inserted.trim().to_owned();
            let editor = sequence_editor_command()?;

            let mut rebase = Command::new("git");
            rebase.args(["rebase", "--interactive"]);
            add_rebase_base(&mut rebase, &target)?;
            rebase
                .env("GIT_SEQUENCE_EDITOR", editor)
                .env("GUT_SEQUENCE_MODE", mode)
                .env("GUT_SEQUENCE_TARGET", &target)
                .env("GUT_SEQUENCE_INSERT", &inserted);
            run_git_command(&mut rebase, "history rewrite", quiet)?;
        }
    }

    let head_after = git_output(&["rev-parse", "HEAD"])?;
    Ok(CommitPlacementResult {
        operation,
        target_before: target,
        head_before: original_head,
        head_after: head_after.trim().to_owned(),
    })
}

pub fn edit_rebase_todo(path: &Path) -> Result<(), String> {
    let mode = env::var("GUT_SEQUENCE_MODE").map_err(|_| "missing sequence mode".to_owned())?;
    let target =
        env::var("GUT_SEQUENCE_TARGET").map_err(|_| "missing sequence target".to_owned())?;
    let inserted =
        env::var("GUT_SEQUENCE_INSERT").map_err(|_| "missing inserted commit".to_owned())?;

    let content =
        fs::read_to_string(path).map_err(|error| format!("failed to read rebase plan: {error}"))?;
    let mut commands = Vec::new();
    let mut suffix = Vec::new();

    for line in content.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            suffix.push(line.to_owned());
        } else {
            commands.push(line.to_owned());
        }
    }

    let insert_index = find_commit_line(&commands, &inserted)
        .ok_or_else(|| "could not find inserted commit in rebase plan".to_owned())?;
    let insert_line = commands.remove(insert_index);
    let target_index = find_commit_line(&commands, &target)
        .ok_or_else(|| "could not find target commit in rebase plan".to_owned())?;

    let destination = match mode.as_str() {
        "before" => target_index,
        "after" => target_index + 1,
        _ => return Err(format!("unsupported sequence mode: {mode}")),
    };
    commands.insert(destination, insert_line);

    let mut rewritten = commands.join("\n");
    if !suffix.is_empty() {
        rewritten.push('\n');
        rewritten.push_str(&suffix.join("\n"));
    }
    rewritten.push('\n');

    fs::write(path, rewritten).map_err(|error| format!("failed to write rebase plan: {error}"))
}

fn find_commit_line(lines: &[String], full_hash: &str) -> Option<usize> {
    lines.iter().position(|line| {
        let mut fields = line.split_whitespace();
        let _command = fields.next();
        fields
            .next()
            .is_some_and(|abbrev| full_hash.starts_with(abbrev))
    })
}

fn sequence_editor_command() -> Result<String, String> {
    let exe = env::current_exe().map_err(|error| format!("failed to locate gut: {error}"))?;
    Ok(format!(
        "{} __sequence-editor",
        shell_quote(&exe.to_string_lossy())
    ))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn require_attached_branch() -> Result<(), String> {
    let status = Command::new("git")
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to inspect HEAD: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("commit placement requires an attached branch".to_owned())
    }
}

fn require_ancestor(target: &str, head: &str) -> Result<(), String> {
    if git_status(&["merge-base", "--is-ancestor", target, head])? {
        Ok(())
    } else {
        Err("target commit must be an ancestor of HEAD".to_owned())
    }
}

fn require_linear_history(target: &str, head: &str) -> Result<(), String> {
    let range = format!("{target}..{head}");
    let merges = git_output(&["rev-list", "--merges", &range])?;
    let target_parents = git_output(&["rev-list", "--parents", "-n", "1", target])?;
    let target_is_merge = target_parents.split_whitespace().count() > 2;

    if target_is_merge || !merges.trim().is_empty() {
        Err("commit placement currently supports linear history only".to_owned())
    } else {
        Ok(())
    }
}

fn add_rebase_base(command: &mut Command, target: &str) -> Result<(), String> {
    let parent = git_output(&["rev-parse", "--verify", &format!("{target}^")]);
    match parent {
        Ok(parent) => {
            command.arg(parent.trim());
        }
        Err(_) => {
            command.arg("--root");
        }
    }
    Ok(())
}

fn git(args: &[&str]) -> Result<(), String> {
    let mut command = Command::new("git");
    command.args(args);
    run_git_command(&mut command, "git command", false)
}

fn git_status(args: &[&str]) -> Result<bool, String> {
    Command::new("git")
        .args(args)
        .status()
        .map(|status| status.success())
        .map_err(|error| format!("failed to run git: {error}"))
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

fn run_git_command(command: &mut Command, operation: &str, quiet: bool) -> Result<(), String> {
    if quiet {
        command.stdout(Stdio::null());
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run {operation}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{operation} failed"))
    }
}
