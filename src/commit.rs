use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
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
    pub operation_id: String,
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
    selection: &crate::changes::Selection,
) -> Result<CommitPlacementResult, String> {
    let (target_input, mode, placement_kind) = match &placement {
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
    require_supported_target(&target)?;
    let preserve_merges = history_contains_merges(&target, &original_head)?;

    let selective = if selection.is_empty() {
        None
    } else {
        let state = crate::changes::prepare_selection(selection)?;
        if state.original_head() != original_head {
            return Err("repository HEAD changed while preparing selected changes".to_owned());
        }
        if let Err(error) = state.isolate_selected() {
            return match state.restore_original() {
                Ok(()) => Err(error),
                Err(restore_error) => Err(format!(
                    "{error}; failed to restore original change state: {restore_error}"
                )),
            };
        }
        state.preflight_residual()?;
        Some(state)
    };

    let index_snapshot = if selective.is_none() {
        Some(IndexSnapshot::capture()?)
    } else {
        None
    };

    if selective.is_none()
        && let Err(error) = git(&["add", "-A"])
    {
        if let Some(snapshot) = &index_snapshot
            && let Err(restore_error) = snapshot.restore()
        {
            return Err(format!(
                "{error}; failed to restore Git index: {restore_error}"
            ));
        }
        return Err(error);
    }

    if git_status(&["diff", "--cached", "--quiet"])? {
        if let Some(state) = &selective {
            state.restore_original()?;
        } else if let Some(snapshot) = &index_snapshot {
            snapshot.restore()?;
        }
        return Err("there are no changes to commit".to_owned());
    }

    let operation_kind = match placement_kind {
        PlacementKind::Update => crate::operation::OperationKind::Update,
        PlacementKind::Before => crate::operation::OperationKind::Before,
        PlacementKind::After => crate::operation::OperationKind::After,
    };
    let pending = match crate::operation::begin(operation_kind, Some(target.clone())) {
        Ok(pending) => pending,
        Err(error) => {
            if let Some(state) = &selective {
                if let Err(restore_error) = state.restore_original() {
                    return Err(format!(
                        "{error}; failed to restore original change state: {restore_error}"
                    ));
                }
            } else if let Some(snapshot) = &index_snapshot
                && let Err(restore_error) = snapshot.restore()
            {
                return Err(format!(
                    "{error}; failed to restore Git index: {restore_error}"
                ));
            }
            return Err(error);
        }
    };

    let rewrite = (|| -> Result<(), String> {
        match placement {
            Placement::Update(_) => {
                let mut commit = Command::new("git");
                commit.args(["commit", "--fixup", &target]);
                run_git_command(&mut commit, "commit", quiet)?;
                let mut rebase = Command::new("git");
                rebase.arg("rebase");
                if preserve_merges {
                    rebase.arg("--rebase-merges");
                }
                rebase.args(["--interactive", "--autosquash"]);
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
                rebase.arg("rebase");
                if preserve_merges {
                    rebase.arg("--rebase-merges");
                }
                rebase.arg("--interactive");
                add_rebase_base(&mut rebase, &target)?;
                rebase
                    .env("GIT_SEQUENCE_EDITOR", editor)
                    .env("GUT_SEQUENCE_MODE", mode)
                    .env("GUT_SEQUENCE_TARGET", &target)
                    .env("GUT_SEQUENCE_INSERT", &inserted);
                run_git_command(&mut rebase, "history rewrite", quiet)?;
            }
        }
        Ok(())
    })();

    if let Err(error) = rewrite {
        let rollback = if let Some(state) = &selective {
            state.restore_original()
        } else {
            rollback_failed_rewrite(
                &original_head,
                index_snapshot
                    .as_ref()
                    .ok_or_else(|| "missing index rollback snapshot".to_owned())?,
            )
        };
        if let Err(rollback_error) = rollback {
            return Err(format!(
                "{error}; automatic rollback failed: {rollback_error}; original HEAD is protected at {}",
                pending.recovery_ref()
            ));
        }
        pending.abort();
        return Err(error);
    }

    if let Some(state) = &selective
        && let Err(error) = state.restore_residual()
    {
        match state.restore_original() {
            Ok(()) => {
                pending.abort();
                return Err(format!(
                    "history rewrite succeeded but remaining changes could not be restored; the rewrite was rolled back: {error}"
                ));
            }
            Err(rollback_error) => {
                return Err(format!(
                    "history rewrite succeeded but remaining changes could not be restored: {error}; rollback failed: {rollback_error}; original HEAD is protected at {}",
                    pending.recovery_ref()
                ));
            }
        }
    }

    let operation_record = pending.finish()?;
    let head_after = git_output(&["rev-parse", "HEAD"])?;
    Ok(CommitPlacementResult {
        operation: placement_kind,
        target_before: target,
        head_before: original_head,
        head_after: head_after.trim().to_owned(),
        operation_id: operation_record.id,
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

fn require_supported_target(target: &str) -> Result<(), String> {
    let target_parents = git_output(&["rev-list", "--parents", "-n", "1", target])?;
    if target_parents.split_whitespace().count() > 2 {
        Err(
            "target commit is a merge commit; choose a non-merge commit because placement relative to a merge is ambiguous"
                .to_owned(),
        )
    } else {
        Ok(())
    }
}

fn history_contains_merges(target: &str, head: &str) -> Result<bool, String> {
    let range = format!("{target}..{head}");
    Ok(!git_output(&["rev-list", "--merges", &range])?
        .trim()
        .is_empty())
}

struct IndexSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl IndexSnapshot {
    fn capture() -> Result<Self, String> {
        let path = git_output(&["rev-parse", "--git-path", "index"])?;
        let path = PathBuf::from(path.trim());
        let path = if path.is_absolute() {
            path
        } else {
            env::current_dir()
                .map_err(|error| format!("failed to inspect current directory: {error}"))?
                .join(path)
        };
        let contents = match fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "failed to snapshot Git index {}: {error}",
                    path.display()
                ));
            }
        };
        Ok(Self { path, contents })
    }

    fn restore(&self) -> Result<(), String> {
        match &self.contents {
            Some(contents) => fs::write(&self.path, contents)
                .map_err(|error| format!("failed to restore Git index: {error}")),
            None => match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("failed to restore Git index: {error}")),
            },
        }
    }
}

fn rollback_failed_rewrite(
    original_head: &str,
    index_snapshot: &IndexSnapshot,
) -> Result<(), String> {
    let _ = Command::new("git")
        .args(["rebase", "--abort"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let reset = Command::new("git")
        .args(["reset", "--mixed", original_head])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to restore HEAD: {error}"))?;
    if !reset.success() {
        return Err("failed to restore HEAD and working tree".to_owned());
    }
    index_snapshot.restore()
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
