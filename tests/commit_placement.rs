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
        "gut-commit-placement-{}-{unique}",
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
            let (hash, found_subject) = line.split_once('\t')?;
            (found_subject == subject).then(|| hash.to_owned())
        })
        .expect("commit subject")
}

fn subjects(repo: &Path) -> Vec<String> {
    let mut values: Vec<_> = git(repo, &["log", "--format=%s"])
        .lines()
        .map(str::to_owned)
        .collect();
    values.reverse();
    values
}

fn gut_output(repo: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(repo)
        .args(args)
        .env("GIT_EDITOR", "true")
        .output()
        .expect("run gut");
    assert!(
        output.status.success(),
        "gut {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn gut(repo: &Path, args: &[&str]) {
    gut_output(repo, args);
}

#[test]
fn update_rewrites_target_and_descendants() {
    let repo = repo();
    let old_b = commit(&repo, "B");
    let old_head = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("placement.txt"), "update\n").unwrap();

    gut(&repo, &["commit", "--update", &old_b]);

    assert_eq!(subjects(&repo), ["A", "B", "C", "D"]);
    let new_b = commit(&repo, "B");
    assert_ne!(new_b, old_b);
    assert_ne!(git(&repo, &["rev-parse", "HEAD"]), old_head);
    assert_eq!(
        git(&repo, &["show", &format!("{new_b}:placement.txt")]),
        "update"
    );
    assert!(git(&repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn before_inserts_commit_before_target() {
    let repo = repo();
    let b = commit(&repo, "B");
    fs::write(repo.join("placement.txt"), "before\n").unwrap();

    gut(&repo, &["commit", "--before", &b, "-m", "X"]);

    assert_eq!(subjects(&repo), ["A", "X", "B", "C", "D"]);
    let x = commit(&repo, "X");
    assert_eq!(
        git(&repo, &["show", &format!("{x}:placement.txt")]),
        "before"
    );
    assert!(git(&repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn after_inserts_commit_after_target() {
    let repo = repo();
    let b = commit(&repo, "B");
    fs::write(repo.join("placement.txt"), "after\n").unwrap();

    gut(&repo, &["commit", "--after", &b, "-m", "X"]);

    assert_eq!(subjects(&repo), ["A", "B", "X", "C", "D"]);
    let x = commit(&repo, "X");
    assert_eq!(
        git(&repo, &["show", &format!("{x}:placement.txt")]),
        "after"
    );
    assert!(git(&repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn json_output_is_versioned_and_structured() {
    let repo = repo();
    let old_b = commit(&repo, "B");
    let old_head = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("placement.txt"), "json\n").unwrap();

    let output = gut_output(&repo, &["--format", "json", "commit", "--update", &old_b]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");

    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["data"]["operation"], "update");
    assert_eq!(value["data"]["targetBefore"], old_b);
    assert_eq!(value["data"]["headBefore"], old_head);
    assert_eq!(
        value["data"]["headAfter"],
        git(&repo, &["rev-parse", "HEAD"])
    );
}
