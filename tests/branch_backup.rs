use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
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
        std::env::temp_dir().join(format!("gut-branch-backup-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);
    fs::write(path.join("A.txt"), "A\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "A"]);
    git(&path, &["switch", "-c", "feature/nested"]);
    path
}

fn gut(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run gut")
}

#[test]
fn backup_creates_matching_prefixed_branch_and_switches_to_it() {
    let repo = repo();
    let head = git(&repo, &["rev-parse", "HEAD"]);

    let output = gut(&repo, &["branch", "backup"]);
    assert!(
        output.status.success(),
        "gut failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        git(&repo, &["branch", "--show-current"]),
        "backup/feature/nested"
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&repo, &["rev-parse", "backup/feature/nested"]), head);
}

#[test]
fn backup_push_pushes_to_origin_and_sets_upstream() {
    let repo = repo();
    let remote = repo.with_extension("remote.git");
    fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );

    let output = gut(&repo, &["branch", "backup", "--push"]);
    assert!(
        output.status.success(),
        "gut failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        git(&repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/backup/feature/nested"
    );
    assert_eq!(
        git(&remote, &["rev-parse", "refs/heads/backup/feature/nested"]),
        git(&repo, &["rev-parse", "HEAD"])
    );
}

#[test]
fn backup_refuses_to_overwrite_an_existing_backup_branch() {
    let repo = repo();
    git(&repo, &["branch", "backup/feature/nested"]);

    let output = gut(&repo, &["branch", "backup"]);
    assert!(!output.status.success());
    assert_eq!(git(&repo, &["branch", "--show-current"]), "feature/nested");
}

#[test]
fn backup_refuses_detached_head() {
    let repo = repo();
    let head = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["switch", "--detach", &head]);

    let output = gut(&repo, &["branch", "backup"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot back up a detached HEAD"));
}
