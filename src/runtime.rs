use std::{
    io::{self, BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{changes, commit, log, operation, plan, repository, review};

#[derive(Deserialize)]
struct Request {
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Default, Deserialize)]
struct StatusParams {
    remote: Option<String>,
    main: Option<String>,
    #[serde(default)]
    local: bool,
}

#[derive(Default, Deserialize)]
struct ReviewParams {
    base: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitPlaceParams {
    mode: String,
    target: String,
    message: Option<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    hunks: Vec<String>,
    expected_state: Option<String>,
    expected_head: Option<String>,
}

#[derive(Deserialize)]
struct PlanApplyParams {
    plan: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationParams {
    operation: Option<String>,
    expected_state: Option<String>,
    expected_head: Option<String>,
}

struct RuntimeError {
    code: &'static str,
    message: String,
    data: Option<Value>,
}

impl From<String> for RuntimeError {
    fn from(message: String) -> Self {
        Self {
            code: "runtime_error",
            message,
            data: None,
        }
    }
}

impl From<repository::StateMismatch> for RuntimeError {
    fn from(mismatch: repository::StateMismatch) -> Self {
        let message = mismatch.message();
        let data = serde_json::to_value(&mismatch).ok();
        Self {
            code: "stale_repository_state",
            message,
            data,
        }
    }
}

type SharedStdout = Arc<Mutex<io::Stdout>>;
type SharedCore = Arc<Mutex<()>>;

pub fn run(watch_interval_ms: u64) -> Result<(), String> {
    let stdout = Arc::new(Mutex::new(io::stdout()));
    let core = Arc::new(Mutex::new(()));
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = if watch_interval_ms == 0 {
        None
    } else {
        let initial_repository = repository::inspect().ok();
        let initial_operation = operation::log()
            .ok()
            .and_then(|records| records.first().map(|record| record.id.clone()));
        Some(spawn_watcher(
            stdout.clone(),
            core.clone(),
            stop.clone(),
            Duration::from_millis(watch_interval_ms),
            initial_repository,
            initial_operation,
        ))
    };

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| format!("failed to read runtime request: {error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        handle_line(&stdout, &core, &line)?;
    }

    stop.store(true, Ordering::Relaxed);
    if let Some(watcher) = watcher {
        watcher.thread().unpark();
        watcher
            .join()
            .map_err(|_| "runtime watcher thread panicked".to_owned())?;
    }
    Ok(())
}

fn handle_line(stdout: &SharedStdout, core: &SharedCore, line: &str) -> Result<(), String> {
    let request = match serde_json::from_str::<Request>(line) {
        Ok(request) => request,
        Err(error) => {
            return write_value(
                stdout,
                &json!({
                    "schemaVersion": 1,
                    "id": Value::Null,
                    "error": { "message": format!("invalid request: {error}") }
                }),
            );
        }
    };

    let id = request.id.clone();
    let result = {
        let _guard = core
            .lock()
            .map_err(|_| "runtime core lock was poisoned".to_owned())?;
        dispatch(&request.method, request.params)
    };
    let response = match result {
        Ok(result) => json!({
            "schemaVersion": 1,
            "id": id,
            "result": result
        }),
        Err(error) => json!({
            "schemaVersion": 1,
            "id": id,
            "error": {
                "code": error.code,
                "message": error.message,
                "data": error.data
            }
        }),
    };
    write_value(stdout, &response)
}

fn dispatch(method: &str, params: Value) -> Result<Value, RuntimeError> {
    match method {
        "runtime.describe" => Ok(json!({
            "protocolVersion": 1,
            "transport": "ndjson-stdio",
            "methods": [
                "runtime.describe",
                "repository.get",
                "changes.get",
                "status.get",
                "log.get",
                "review.get",
                "commit.plan",
                "commit.place",
                "plan.apply",
                "operation.log",
                "operation.diff",
                "operation.undo"
            ],
            "events": [
                "repository.changed",
                "workingTree.changed",
                "operation.completed"
            ],
            "commitPlacementModes": ["before", "update", "after"],
            "mutationGuards": ["expectedState", "expectedHead"]
        })),
        "repository.get" => to_value(repository::inspect()?),
        "changes.get" => to_value(changes::inspect()?),
        "status.get" => {
            let params: StatusParams = decode_default(params)?;
            let remote = params.remote.as_deref().unwrap_or("origin");
            let main = params.main.as_deref().unwrap_or("main");
            to_value(crate::StatusOutput {
                branches: crate::branch_status(remote, main, params.local)?,
                worktrees: crate::worktree_status()?,
            })
        }
        "log.get" => to_value(log::inspect()?),
        "review.get" => {
            let params: ReviewParams = decode_default(params)?;
            to_value(review::inspect(
                params.base.as_deref().unwrap_or("origin/main"),
            )?)
        }
        "commit.plan" => {
            let params: CommitPlaceParams = decode(params)?;
            let guard = repository::MutationGuard {
                expected_state: params.expected_state.clone(),
                expected_head: params.expected_head.clone(),
            };
            repository::check_guard(&guard)?;
            let placement = placement_from_runtime(&params.mode, params.target)?;
            to_value(plan::create(
                &placement,
                params.message.as_deref(),
                &changes::Selection {
                    files: params.files,
                    hunks: params.hunks,
                },
            )?)
        }
        "commit.place" => {
            let params: CommitPlaceParams = decode(params)?;
            let guard = repository::MutationGuard {
                expected_state: params.expected_state.clone(),
                expected_head: params.expected_head.clone(),
            };
            repository::check_guard(&guard)?;
            let placement = placement_from_runtime(&params.mode, params.target)?;
            match commit::place_current_changes(
                placement,
                params.message.as_deref(),
                true,
                &changes::Selection {
                    files: params.files,
                    hunks: params.hunks,
                },
                &guard,
            ) {
                Ok(result) => to_value(result),
                Err(error) if error.starts_with("stale repository state:") => {
                    match repository::check_guard(&guard) {
                        Err(mismatch) => Err(mismatch.into()),
                        Ok(()) => Err(error.into()),
                    }
                }
                Err(error) => Err(error.into()),
            }
        }
        "plan.apply" => {
            let params: PlanApplyParams = decode(params)?;
            let stored = plan::load(&params.plan)?;
            let guard = repository::MutationGuard {
                expected_state: Some(stored.state_token),
                expected_head: Some(stored.head_before),
            };
            repository::check_guard(&guard)?;
            to_value(plan::apply(&params.plan, true)?)
        }
        "operation.log" => to_value(operation::log()?),
        "operation.diff" => {
            let params: OperationParams = decode_default(params)?;
            let id = params
                .operation
                .as_deref()
                .ok_or_else(|| "operation.diff requires params.operation".to_owned())?;
            to_value(operation::diff(id)?)
        }
        "operation.undo" => {
            let params: OperationParams = decode_default(params)?;
            let guard = repository::MutationGuard {
                expected_state: params.expected_state,
                expected_head: params.expected_head,
            };
            repository::check_guard(&guard)?;
            match operation::undo(params.operation.as_deref(), &guard) {
                Ok(result) => to_value(result),
                Err(error) if error.starts_with("stale repository state:") => {
                    match repository::check_guard(&guard) {
                        Err(mismatch) => Err(mismatch.into()),
                        Ok(()) => Err(error.into()),
                    }
                }
                Err(error) => Err(error.into()),
            }
        }
        _ => Err(format!("unknown runtime method: {method}").into()),
    }
}

fn placement_from_runtime(mode: &str, target: String) -> Result<commit::Placement, RuntimeError> {
    match mode {
        "update" => Ok(commit::Placement::Update(target)),
        "before" => Ok(commit::Placement::Before(target)),
        "after" => Ok(commit::Placement::After(target)),
        mode => Err(format!("unsupported commit placement mode: {mode}").into()),
    }
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, RuntimeError> {
    serde_json::from_value(value).map_err(|error| format!("invalid params: {error}").into())
}

fn decode_default<T: DeserializeOwned + Default>(value: Value) -> Result<T, RuntimeError> {
    if value.is_null() {
        Ok(T::default())
    } else {
        decode(value)
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, RuntimeError> {
    serde_json::to_value(value)
        .map_err(|error| format!("failed to serialize runtime result: {error}").into())
}

fn spawn_watcher(
    stdout: SharedStdout,
    core: SharedCore,
    stop: Arc<AtomicBool>,
    interval: Duration,
    mut previous_repository: Option<repository::RepositoryState>,
    mut previous_operation: Option<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            thread::park_timeout(interval);
            if stop.load(Ordering::Relaxed) {
                break;
            }

            let (next_repository, records) = {
                let Ok(_guard) = core.lock() else {
                    return;
                };
                (repository::inspect().ok(), operation::log().ok())
            };

            if let Some(next) = next_repository {
                if let Some(previous) = previous_repository.as_ref() {
                    if (previous.root != next.root
                        || previous.branch != next.branch
                        || previous.head != next.head)
                        && emit_event(&stdout, "repository.changed", &next).is_err()
                    {
                        break;
                    }
                    let same_history = previous.branch == next.branch && previous.head == next.head;
                    let worktree_changed = previous.worktree != next.worktree
                        || (same_history && previous.state_token != next.state_token);
                    if worktree_changed
                        && emit_event(&stdout, "workingTree.changed", &next).is_err()
                    {
                        break;
                    }
                }
                previous_repository = Some(next);
            }

            if let Some(records) = records {
                let mut new_records = Vec::new();
                for record in &records {
                    if previous_operation.as_deref() == Some(record.id.as_str()) {
                        break;
                    }
                    new_records.push(record.clone());
                }
                for record in new_records.into_iter().rev() {
                    if emit_event(&stdout, "operation.completed", &record).is_err() {
                        return;
                    }
                }
                previous_operation = records.first().map(|record| record.id.clone());
            }
        }
    })
}

fn emit_event<T: serde::Serialize>(
    stdout: &SharedStdout,
    event: &str,
    data: &T,
) -> Result<(), String> {
    write_value(
        stdout,
        &json!({
            "schemaVersion": 1,
            "event": event,
            "data": serde_json::to_value(data)
                .map_err(|error| format!("failed to serialize runtime event: {error}"))?
        }),
    )
}

fn write_value(stdout: &SharedStdout, value: &Value) -> Result<(), String> {
    let mut stdout = stdout
        .lock()
        .map_err(|_| "runtime stdout lock was poisoned".to_owned())?;
    serde_json::to_writer(&mut *stdout, value)
        .map_err(|error| format!("failed to write runtime response: {error}"))?;
    stdout
        .write_all(b"\n")
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("failed to flush runtime response: {error}"))
}
