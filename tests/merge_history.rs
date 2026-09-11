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

fn git_succeeds(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(repo)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run git")
        .success()
}

fn fresh_repo(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gut-{prefix}-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);
    path
}

fn add_commit(repo: &Path, subject: &str) -> String {
    fs::write(repo.join(format!("{subject}.txt")), format!("{subject}\n")).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", subject]);
    git(repo, &["rev-parse", "HEAD"])
}

fn merge_repo() -> PathBuf {
    let repo = fresh_repo("merge-history");
    add_commit(&repo, "A");
    add_commit(&repo, "B");

    git(&repo, &["switch", "-c", "side"]);
    add_commit(&repo, "D-side");

    git(&repo, &["switch", "main"]);
    add_commit(&repo, "C-main");
    git(&repo, &["merge", "--no-ff", "side", "-m", "Merge side"]);
    add_commit(&repo, "F");
    repo
}

fn commit(repo: &Path, subject: &str) -> String {
    git(repo, &["log", "HEAD", "--format=%H%x09%s"])
        .lines()
        .find_map(|line| {
            let (hash, found_subject) = line.split_once("\t")?;
            (found_subject == subject).then(|| hash.to_owned())
        })
        .expect("commit subject")
}

fn first_parent_subjects(repo: &Path) -> Vec<String> {
    let mut subjects = git(repo, &["log", "--first-parent", "--format=%s"])
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    subjects.reverse();
    subjects
}

fn gut_output(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(repo)
        .args(args)
        .env("GIT_EDITOR", "true")
        .output()
        .expect("run gut")
}

fn gut(repo: &Path, args: &[&str]) {
    let output = gut_output(repo, args);
    assert!(
        output.status.success(),
        "gut {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_single_merge(repo: &Path) -> String {
    assert_eq!(git(repo, &["rev-list", "--merges", "--count", "HEAD"]), "1");
    let merge = commit(repo, "Merge side");
    assert_eq!(
        git(repo, &["rev-list", "--parents", "-n", "1", &merge])
            .split_whitespace()
            .count(),
        3
    );
    merge
}

#[test]
fn update_preserves_merge_topology() {
    let repo = merge_repo();
    let old_b = commit(&repo, "B");
    let old_merge = commit(&repo, "Merge side");
    fs::write(repo.join("placement.txt"), "update\n").unwrap();

    gut(&repo, &["commit", "--update", &old_b]);

    assert_eq!(
        first_parent_subjects(&repo),
        ["A", "B", "C-main", "Merge side", "F"]
    );
    let new_b = commit(&repo, "B");
    let new_merge = assert_single_merge(&repo);
    assert_ne!(new_b, old_b);
    assert_ne!(new_merge, old_merge);
    assert_eq!(
        git(&repo, &["show", &format!("{new_b}:placement.txt")]),
        "update"
    );
    assert_eq!(
        git(&repo, &["show", &format!("{new_merge}^2:placement.txt")]),
        "update"
    );
}

#[test]
fn before_and_after_preserve_merge_topology() {
    for (mode, expected) in [
        ("--before", vec!["A", "B", "X", "C-main", "Merge side", "F"]),
        ("--after", vec!["A", "B", "C-main", "X", "Merge side", "F"]),
    ] {
        let repo = merge_repo();
        let c = commit(&repo, "C-main");
        fs::write(repo.join("placement.txt"), format!("{mode}\n")).unwrap();

        gut(&repo, &["commit", mode, &c, "-m", "X"]);

        assert_eq!(first_parent_subjects(&repo), expected);
        let merge = assert_single_merge(&repo);
        assert!(!git_succeeds(
            &repo,
            &["cat-file", "-e", &format!("{merge}^2:placement.txt")]
        ));
        assert_eq!(
            git(&repo, &["show", &format!("{merge}:placement.txt")]),
            mode
        );
    }
}

#[test]
fn updating_a_side_branch_commit_preserves_the_merge() {
    let repo = merge_repo();
    let side = commit(&repo, "D-side");
    fs::write(repo.join("side-placement.txt"), "side update\n").unwrap();

    gut(&repo, &["commit", "--update", &side]);

    let merge = assert_single_merge(&repo);
    let side_after = commit(&repo, "D-side");
    assert_ne!(side_after, side);
    assert_eq!(
        git(&repo, &["show", &format!("{merge}^2:side-placement.txt")]),
        "side update"
    );
}

#[test]
fn merge_commit_targets_are_rejected_without_touching_changes() {
    let repo = merge_repo();
    let merge = commit(&repo, "Merge side");
    fs::write(repo.join("placement.txt"), "keep me\n").unwrap();

    let output = gut_output(&repo, &["commit", "--after", &merge, "-m", "X"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("target commit is a merge commit"));
    assert_eq!(
        fs::read_to_string(repo.join("placement.txt")).unwrap(),
        "keep me\n"
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "?? placement.txt");
}

#[test]
fn failed_rewrite_restores_head_worktree_and_index() {
    let repo = fresh_repo("rollback");
    fs::write(repo.join("conflict.txt"), "A\n").unwrap();
    fs::write(repo.join("staged.txt"), "base\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "A"]);

    fs::write(repo.join("conflict.txt"), "B\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "B"]);
    let b = git(&repo, &["rev-parse", "HEAD"]);

    fs::write(repo.join("conflict.txt"), "C\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "C"]);
    let original_head = git(&repo, &["rev-parse", "HEAD"]);

    fs::write(repo.join("staged.txt"), "staged change\n").unwrap();
    git(&repo, &["add", "staged.txt"]);
    fs::write(repo.join("conflict.txt"), "working change\n").unwrap();

    let output = gut_output(&repo, &["commit", "--update", &b]);

    assert!(!output.status.success(), "rewrite unexpectedly succeeded");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), original_head);
    assert_eq!(
        fs::read_to_string(repo.join("conflict.txt")).unwrap(),
        "working change\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("staged.txt")).unwrap(),
        "staged change\n"
    );
    assert_eq!(
        git(&repo, &["diff", "--cached", "--name-only"]),
        "staged.txt"
    );
    assert_eq!(git(&repo, &["diff", "--name-only"]), "conflict.txt");
    assert!(!repo.join(".git").join("rebase-merge").exists());
    assert!(!repo.join(".git").join("rebase-apply").exists());
}
