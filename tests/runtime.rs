use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8 git output")
        .trim()
        .to_owned()
}

fn repo() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gut-runtime-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);

    fs::write(path.join("A.txt"), "A\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "A"]);
    git(&path, &["switch", "-c", "feature"]);
    fs::write(path.join("B.txt"), "B\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "B"]);
    path
}

fn send(stdin: &mut impl Write, request: Value) {
    serde_json::to_writer(&mut *stdin, &request).expect("write request");
    stdin.write_all(b"\n").expect("write newline");
    stdin.flush().expect("flush request");
}

fn responses_by_id(lines: &[Value]) -> BTreeMap<String, &Value> {
    lines
        .iter()
        .filter_map(|line| {
            line.get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_owned(), line))
        })
        .collect()
}

#[test]
fn runtime_keeps_one_process_for_queries_mutations_and_events() {
    let repo = repo();
    let original_head = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("runtime-change.txt"), "runtime change\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(&repo)
        .args(["runtime", "--watch-interval-ms", "20"])
        .env("GIT_EDITOR", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn runtime");
    let mut stdin = child.stdin.take().expect("runtime stdin");

    send(&mut stdin, json!({"id":"repo","method":"repository.get"}));
    send(
        &mut stdin,
        json!({"id":"status","method":"status.get","params":{"local":true}}),
    );
    send(&mut stdin, json!({"id":"log","method":"log.get"}));
    send(
        &mut stdin,
        json!({"id":"review","method":"review.get","params":{"base":"main"}}),
    );
    send(
        &mut stdin,
        json!({
            "id":"place",
            "method":"commit.place",
            "params":{"mode":"after","target":original_head,"message":"X"}
        }),
    );

    thread::sleep(Duration::from_millis(150));
    send(
        &mut stdin,
        json!({"id":"operations","method":"operation.log"}),
    );
    send(
        &mut stdin,
        json!({"id":"undo","method":"operation.undo","params":{}}),
    );
    thread::sleep(Duration::from_millis(150));
    drop(stdin);

    let output = child.wait_with_output().expect("wait for runtime");
    assert!(
        output.status.success(),
        "runtime failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 runtime output");
    let lines = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("valid runtime json line"))
        .collect::<Vec<_>>();
    assert!(lines.iter().all(|line| line["schemaVersion"] == 1));

    let responses = responses_by_id(&lines);
    let repo_response = responses.get("repo").expect("repository response");
    assert_eq!(repo_response["result"]["branch"], "feature");
    assert_eq!(repo_response["result"]["head"], original_head);
    assert_eq!(repo_response["result"]["dirty"], true);

    let status = responses.get("status").expect("status response");
    assert!(status["result"]["branches"]["would_change_main"].is_array());

    let log = responses.get("log").expect("log response");
    assert_eq!(log["result"]["branch"], "feature");
    assert_eq!(log["result"]["commits"][0]["subject"], "B");

    let review = responses.get("review").expect("review response");
    assert_eq!(review["result"]["branch"], "feature");
    assert_eq!(review["result"]["commits"][0]["subject"], "B");

    let place = responses.get("place").expect("placement response");
    assert_eq!(place["result"]["operation"], "after");
    let operation_id = place["result"]["operationId"]
        .as_str()
        .expect("operation id");

    let operations = responses.get("operations").expect("operation log response");
    assert_eq!(operations["result"][0]["id"], operation_id);

    let undo = responses.get("undo").expect("undo response");
    assert_eq!(undo["result"]["kind"], "undo");
    assert_eq!(undo["result"]["target"], operation_id);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), original_head);
    assert!(!repo.join("runtime-change.txt").exists());

    let events = lines
        .iter()
        .filter_map(|line| line.get("event").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(events.contains(&"repository.changed"), "events: {events:?}");
    assert!(
        events.contains(&"workingTree.changed"),
        "events: {events:?}"
    );
    assert!(
        events.contains(&"operation.completed"),
        "events: {events:?}"
    );
}

#[test]
fn runtime_reports_protocol_errors_without_exiting() {
    let repo = repo();
    let mut child = Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(&repo)
        .args(["runtime", "--watch-interval-ms", "0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn runtime");
    let mut stdin = child.stdin.take().expect("runtime stdin");

    send(&mut stdin, json!({"id":"bad","method":"does.not.exist"}));
    send(&mut stdin, json!({"id":"good","method":"repository.get"}));
    drop(stdin);

    let output = child.wait_with_output().expect("wait for runtime");
    assert!(output.status.success());
    let lines = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let responses = responses_by_id(&lines);
    assert!(
        responses["bad"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown runtime method")
    );
    assert_eq!(responses["good"]["result"]["branch"], "feature");
}
