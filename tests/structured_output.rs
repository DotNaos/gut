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
        "gut-structured-output-{}-{unique}",
        std::process::id()
    ));
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

fn gut_json(repo: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run gut");
    assert!(
        output.status.success(),
        "gut {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid json")
}

#[test]
fn log_json_shortcut_is_versioned_and_structured() {
    let repo = repo();
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let value = gut_json(&repo, &["log", "--json"]);

    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["data"]["branch"], "feature");
    assert_eq!(value["data"]["head"], head);
    let commits = value["data"]["commits"].as_array().expect("commits");
    assert_eq!(commits[0]["subject"], "B");
    assert_eq!(commits[1]["subject"], "A");
    assert_eq!(commits[0]["parents"].as_array().unwrap().len(), 1);
}

#[test]
fn existing_json_commands_use_the_versioned_envelope() {
    let repo = repo();

    let status = gut_json(&repo, &["status", "--local", "--json"]);
    assert_eq!(status["schemaVersion"], 1);
    assert!(status["data"]["branches"]["would_change_main"].is_array());
    assert!(status["data"]["branches"]["wouldChangeMain"].is_null());

    let branches = gut_json(&repo, &["branches", "--local", "--json"]);
    assert_eq!(branches["schemaVersion"], 1);
    assert_eq!(branches["data"]["branches"][0], "feature");

    let worktrees = gut_json(&repo, &["worktrees", "--json"]);
    assert_eq!(worktrees["schemaVersion"], 1);
    assert!(worktrees["data"]["clean"].is_array());
    assert!(worktrees["data"]["dirty"].is_array());
}
