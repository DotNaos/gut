use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

use crate::{
    changes::Selection,
    commit::{CommitPlacementResult, Placement, PlacementKind},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementPlan {
    pub id: String,
    pub state_token: String,
    pub operation: PlacementKind,
    pub target_before: String,
    pub head_before: String,
    pub branch: String,
    pub message: Option<String>,
    pub selection: Selection,
    pub preserve_merges: bool,
    pub affected_commits: Vec<PlannedCommit>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PlannedCommit {
    pub id: String,
    pub parents: Vec<String>,
    pub subject: String,
}

pub fn create(
    placement: &Placement,
    message: Option<&str>,
    selection: &Selection,
) -> Result<PlacementPlan, String> {
    let (operation, target_input) = match placement {
        Placement::Update(target) => (PlacementKind::Update, target.as_str()),
        Placement::Before(target) => (PlacementKind::Before, target.as_str()),
        Placement::After(target) => (PlacementKind::After, target.as_str()),
    };
    if operation == PlacementKind::Update && message.is_some() {
        return Err("--message is only valid with --before or --after".to_owned());
    }

    let target_before = git_output(&[
        "rev-parse",
        "--verify",
        &format!("{target_input}^{{commit}}"),
    ])?;
    let target_before = target_before.trim().to_owned();
    let head_before = git_output(&["rev-parse", "HEAD"])?;
    let head_before = head_before.trim().to_owned();
    let branch = git_output(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| "commit placement requires an attached branch".to_owned())?;
    if !git_success(&["merge-base", "--is-ancestor", &target_before, &head_before])? {
        return Err("target commit must be an ancestor of HEAD".to_owned());
    }
    let target_parents = git_output(&["rev-list", "--parents", "-n", "1", &target_before])?;
    if target_parents.split_whitespace().count() > 2 {
        return Err(
            "target commit is a merge commit; choose a non-merge commit because placement relative to a merge is ambiguous"
                .to_owned(),
        );
    }
    if !selection.is_empty() {
        // Validation only; no repository mutation occurs until isolate_selected is called.
        let _ = crate::changes::prepare_selection(selection)?;
    } else if git_success(&["diff", "--quiet"])?
        && git_success(&["diff", "--cached", "--quiet"])?
        && git_output(&["ls-files", "--others", "--exclude-standard"])?
            .trim()
            .is_empty()
    {
        return Err("there are no changes to commit".to_owned());
    }

    let affected_commits = affected_commits(&target_before, &head_before)?;
    let preserve_merges = affected_commits
        .iter()
        .any(|commit| commit.parents.len() > 1);
    let state_token = crate::repository::state_token()?;
    let mut files = selection.files.clone();
    let mut hunks = selection.hunks.clone();
    files.sort();
    hunks.sort();
    let fingerprint = serde_json::to_vec(&serde_json::json!({
        "stateToken": state_token,
        "operation": operation,
        "target": target_before,
        "message": message,
        "files": files,
        "hunks": hunks,
    }))
    .map_err(|error| error.to_string())?;
    let id = format!("plan-{}", hash_bytes(&fingerprint)?);

    let plan = PlacementPlan {
        id,
        state_token,
        operation,
        target_before,
        head_before,
        branch: branch.trim().to_owned(),
        message: message.map(str::to_owned),
        selection: selection.clone(),
        preserve_merges,
        affected_commits,
    };
    persist(&plan)?;
    Ok(plan)
}

pub fn apply(id: &str, quiet: bool) -> Result<CommitPlacementResult, String> {
    let plan = load(id)?;
    let branch = git_output(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| format!("stale plan {}: HEAD is detached", plan.id))?;
    if branch.trim() != plan.branch {
        return Err(format!(
            "stale plan {}: branch changed (expected {}, actual {})",
            plan.id,
            plan.branch,
            branch.trim()
        ));
    }
    let actual = crate::repository::state_token()?;
    if actual != plan.state_token {
        return Err(format!(
            "stale plan {}: repository state changed (expected {}, actual {})",
            plan.id, plan.state_token, actual
        ));
    }
    let placement = match plan.operation {
        PlacementKind::Update => Placement::Update(plan.target_before.clone()),
        PlacementKind::Before => Placement::Before(plan.target_before.clone()),
        PlacementKind::After => Placement::After(plan.target_before.clone()),
    };
    crate::commit::place_current_changes(placement, plan.message.as_deref(), quiet, &plan.selection)
}

pub fn load(id: &str) -> Result<PlacementPlan, String> {
    validate_id(id)?;
    let path = plan_path(id)?;
    let bytes = fs::read(&path)
        .map_err(|error| format!("failed to read plan {id} from {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid stored plan {id}: {error}"))
}

fn affected_commits(target: &str, head: &str) -> Result<Vec<PlannedCommit>, String> {
    let parent = git_output(&["rev-parse", "--verify", &format!("{target}^")]);
    let range = parent
        .ok()
        .map(|parent| format!("{}..{head}", parent.trim()));
    let mut args = vec![
        "log",
        "--reverse",
        "--topo-order",
        "--format=%H%x09%P%x09%s",
    ];
    if let Some(range) = &range {
        args.push(range);
    } else {
        args.push(head);
    }
    git_output(&args)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.splitn(3, '\t');
            let id = fields
                .next()
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            let parents = fields
                .next()
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            let subject = fields
                .next()
                .ok_or_else(|| "unexpected git log output".to_owned())?;
            Ok(PlannedCommit {
                id: id.to_owned(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                subject: subject.to_owned(),
            })
        })
        .collect()
}

fn persist(plan: &PlacementPlan) -> Result<(), String> {
    let path = plan_path(&plan.id)?;
    let parent = path
        .parent()
        .ok_or_else(|| "invalid plan storage path".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(plan).map_err(|error| error.to_string())?;
    fs::write(&path, bytes)
        .map_err(|error| format!("failed to write plan {}: {error}", path.display()))
}

fn plan_path(id: &str) -> Result<PathBuf, String> {
    let common = git_output(&["rev-parse", "--git-common-dir"])?;
    let common = PathBuf::from(common.trim());
    let common = if common.is_absolute() {
        common
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to inspect current directory: {error}"))?
            .join(common)
    };
    Ok(common.join("gut").join("plans").join(format!("{id}.json")))
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.starts_with("plan-")
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        Ok(())
    } else {
        Err(format!("invalid plan id: {id}"))
    }
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
        .map_err(|error| format!("failed to hash plan: {error}"))?;
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

fn git_success(args: &[&str]) -> Result<bool, String> {
    Command::new("git")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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
