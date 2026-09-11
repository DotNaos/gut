use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

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
    let path =
        std::env::temp_dir().join(format!("gut-concurrency-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);
    fs::write(path.join("work.txt"), "base\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "A"]);
    for name in ["B", "C", "D"] {
        fs::write(path.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", name]);
    }
    path
}

fn commit(repo: &Path, subject: &str) -> String {
    git(repo, &["log", "--format=%H%x09%s"])
        .lines()
        .find_map(|line| {
            let (hash, found) = line.split_once('\t')?;
            (found == subject).then(|| hash.to_owned())
        })
        .expect("commit subject")
}

fn gut_output(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(repo)
        .args(args)
        .env("GIT_EDITOR", "true")
        .output()
        .expect("run gut")
}

fn gut_json(repo: &Path, args: &[&str]) -> serde_json::Value {
    let output = gut_output(repo, args);
    assert!(
        output.status.success(),
        "gut {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid gut json")
}

fn state(repo: &Path) -> serde_json::Value {
    gut_json(repo, &["repository", "--json"])["data"].clone()
}

#[test]
fn state_token_changes_for_branch_head_index_worktree_and_untracked_content() {
    let repo = repo();
    let baseline = state(&repo);
    let baseline_token = baseline["stateToken"].as_str().unwrap().to_owned();
    let head = baseline["head"].as_str().unwrap().to_owned();

    fs::write(repo.join("work.txt"), "unstaged\n").unwrap();
    assert_ne!(state(&repo)["stateToken"], baseline_token);
    git(&repo, &["reset", "--hard", &head]);

    fs::write(repo.join("work.txt"), "staged\n").unwrap();
    git(&repo, &["add", "work.txt"]);
    assert_ne!(state(&repo)["stateToken"], baseline_token);
    git(&repo, &["reset", "--hard", &head]);

    fs::write(repo.join("new.txt"), "one\n").unwrap();
    let untracked_one = state(&repo)["stateToken"].as_str().unwrap().to_owned();
    assert_ne!(untracked_one, baseline_token);
    fs::write(repo.join("new.txt"), "two\n").unwrap();
    let untracked_two = state(&repo)["stateToken"].as_str().unwrap().to_owned();
    assert_ne!(untracked_two, untracked_one);
    fs::remove_file(repo.join("new.txt")).unwrap();
    assert_eq!(state(&repo)["stateToken"], baseline_token);

    git(&repo, &["switch", "-c", "same-head"]);
    assert_ne!(state(&repo)["stateToken"], baseline_token);
    git(&repo, &["switch", "main"]);
    assert_eq!(state(&repo)["stateToken"], baseline_token);

    fs::write(repo.join("head.txt"), "new head\n").unwrap();
    git(&repo, &["add", "head.txt"]);
    git(&repo, &["commit", "-m", "E"]);
    assert_ne!(state(&repo)["stateToken"], baseline_token);
}

#[test]
fn commit_guard_accepts_current_state_and_rejects_stale_state_without_mutation() {
    let success_repo = repo();
    let b = commit(&success_repo, "B");
    fs::write(success_repo.join("work.txt"), "guarded\n").unwrap();
    let current = state(&success_repo);
    let token = current["stateToken"].as_str().unwrap();
    let head = current["head"].as_str().unwrap();
    let output = gut_json(
        &success_repo,
        &[
            "commit",
            "--update",
            &b,
            "--expected-state",
            token,
            "--expected-head",
            head,
            "--json",
        ],
    );
    assert_eq!(output["data"]["operation"], "update");

    let repo = repo();
    let b = commit(&repo, "B");
    fs::write(repo.join("work.txt"), "planned\n").unwrap();
    let observed = state(&repo);
    let expected_state = observed["stateToken"].as_str().unwrap().to_owned();
    let expected_head = observed["head"].as_str().unwrap().to_owned();
    fs::write(repo.join("concurrent.txt"), "external change\n").unwrap();
    let actual_contents = fs::read_to_string(repo.join("work.txt")).unwrap();
    let actual_head = git(&repo, &["rev-parse", "HEAD"]);

    let output = gut_output(
        &repo,
        &[
            "commit",
            "--update",
            &b,
            "--expected-state",
            &expected_state,
            "--expected-head",
            &expected_head,
            "--json",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("stale repository state"),
        "stderr: {stderr}"
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), actual_head);
    assert_eq!(
        fs::read_to_string(repo.join("work.txt")).unwrap(),
        actual_contents
    );
    assert!(repo.join("concurrent.txt").exists());
    assert!(git(&repo, &["for-each-ref", "refs/gut/operations"]).is_empty());
}

#[test]
fn runtime_returns_structured_stale_state_error() {
    let repo = repo();
    let b = commit(&repo, "B");
    fs::write(repo.join("work.txt"), "runtime guarded\n").unwrap();
    let observed = state(&repo);
    let expected_state = observed["stateToken"].as_str().unwrap().to_owned();
    let expected_head = observed["head"].as_str().unwrap().to_owned();
    fs::write(repo.join("external.txt"), "changed after read\n").unwrap();
    let actual = state(&repo);

    let mut child = Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(&repo)
        .args(["runtime", "--watch-interval-ms", "0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn runtime");
    let mut stdin = child.stdin.take().unwrap();
    serde_json::to_writer(
        &mut stdin,
        &serde_json::json!({
            "id":"stale",
            "method":"commit.place",
            "params":{
                "mode":"update",
                "target":b,
                "expectedState":expected_state,
                "expectedHead":expected_head
            }
        }),
    )
    .unwrap();
    stdin.write_all(b"\n").unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: serde_json::Value =
        serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    assert_eq!(response["error"]["code"], "stale_repository_state");
    assert_eq!(response["error"]["data"]["expectedState"], expected_state);
    assert_eq!(
        response["error"]["data"]["actualState"],
        actual["stateToken"]
    );
    assert_eq!(response["error"]["data"]["expectedHead"], expected_head);
    assert_eq!(response["error"]["data"]["actualHead"], actual["head"]);
}

#[test]
fn undo_guard_rejects_stale_head_before_reset() {
    let repo = repo();
    let b = commit(&repo, "B");
    fs::write(repo.join("work.txt"), "placed\n").unwrap();
    let placed = gut_json(&repo, &["commit", "--update", &b, "--json"]);
    let operation = placed["data"]["operationId"].as_str().unwrap().to_owned();
    let current = state(&repo);
    let head = current["head"].as_str().unwrap().to_owned();
    let token = current["stateToken"].as_str().unwrap().to_owned();

    let output = gut_output(
        &repo,
        &[
            "op",
            "undo",
            &operation,
            "--expected-head",
            "0000000000000000000000000000000000000000",
            "--expected-state",
            &token,
        ],
    );
    assert!(!output.status.success());
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);

    let output = gut_output(
        &repo,
        &[
            "op",
            "undo",
            &operation,
            "--expected-head",
            &head,
            "--expected-state",
            &token,
        ],
    );
    assert!(
        output.status.success(),
        "undo failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
