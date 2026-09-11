use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
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
    let path = std::env::temp_dir().join(format!(
        "gut-operation-history-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);

    for name in ["A", "B", "C", "D"] {
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
            let (hash, found_subject) = line.split_once("\t")?;
            (found_subject == subject).then(|| hash.to_owned())
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
    let mut full_args = vec!["--format", "json"];
    full_args.extend_from_slice(args);
    let output = gut_output(repo, &full_args);
    assert!(
        output.status.success(),
        "gut {:?} failed\nstdout:\n{}\nstderr:\n{}",
        full_args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid json")
}

#[test]
fn operations_are_logged_diffable_and_undoable() {
    let repo = repo();
    let old_b = commit(&repo, "B");
    let old_head = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("placement.txt"), "operation history\n").unwrap();

    let placement = gut_json(&repo, &["commit", "--update", &old_b]);
    let operation_id = placement["data"]["operationId"]
        .as_str()
        .expect("operation id")
        .to_owned();
    let new_head = git(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(new_head, old_head);

    let log = gut_json(&repo, &["op", "log"]);
    let record = &log["data"][0];
    assert_eq!(record["id"], operation_id);
    assert_eq!(record["parent"], serde_json::Value::Null);
    assert_eq!(record["kind"], "update");
    assert_eq!(record["branch"], "main");
    assert_eq!(record["target"], old_b);
    assert_eq!(record["headBefore"], old_head);
    assert_eq!(record["headAfter"], new_head);

    let before_ref = record["beforeRef"].as_str().expect("before ref");
    let after_ref = record["afterRef"].as_str().expect("after ref");
    assert_eq!(git(&repo, &["rev-parse", before_ref]), old_head);
    assert_eq!(git(&repo, &["rev-parse", after_ref]), new_head);

    let diff = gut_json(&repo, &["op", "diff", &operation_id]);
    assert_eq!(diff["data"]["operation"]["id"], operation_id);
    assert!(
        diff["data"]["patch"]
            .as_str()
            .expect("patch")
            .contains("placement.txt")
    );

    let undo = gut_json(&repo, &["op", "undo", &operation_id]);
    let undo_id = undo["data"]["id"].as_str().expect("undo id");
    assert_eq!(undo["data"]["kind"], "undo");
    assert_eq!(undo["data"]["target"], operation_id);
    assert_eq!(undo["data"]["headBefore"], new_head);
    assert_eq!(undo["data"]["headAfter"], old_head);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), old_head);
    assert!(!repo.join("placement.txt").exists());

    let log = gut_json(&repo, &["op", "log"]);
    assert_eq!(log["data"][0]["id"], undo_id);
    assert_eq!(log["data"][0]["parent"], operation_id);
    assert_eq!(log["data"][1]["id"], operation_id);
}

#[test]
fn undo_refuses_a_dirty_worktree() {
    let repo = repo();
    let b = commit(&repo, "B");
    fs::write(repo.join("placement.txt"), "update\n").unwrap();
    let placement = gut_json(&repo, &["commit", "--update", &b]);
    let operation_id = placement["data"]["operationId"]
        .as_str()
        .expect("operation id");

    fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();
    let output = gut_output(&repo, &["op", "undo", operation_id]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("undo requires a clean working tree"));
}

#[test]
fn undo_can_restore_an_older_operation_state() {
    let repo = repo();
    let old_b = commit(&repo, "B");
    let original_head = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("first.txt"), "first\n").unwrap();
    let first = gut_json(&repo, &["commit", "--update", &old_b]);
    let first_id = first["data"]["operationId"]
        .as_str()
        .expect("first operation id")
        .to_owned();

    let d = commit(&repo, "D");
    fs::write(repo.join("second.txt"), "second\n").unwrap();
    let second = gut_json(&repo, &["commit", "--after", &d, "-m", "X"]);
    let second_id = second["data"]["operationId"]
        .as_str()
        .expect("second operation id")
        .to_owned();
    assert_ne!(git(&repo, &["rev-parse", "HEAD"]), original_head);

    let undo = gut_json(&repo, &["op", "undo", &first_id]);
    assert_eq!(undo["data"]["target"], first_id);
    assert_eq!(undo["data"]["parent"], second_id);
    assert_eq!(undo["data"]["headAfter"], original_head);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), original_head);
    assert!(!repo.join("first.txt").exists());
    assert!(!repo.join("second.txt").exists());
}
