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
    let path = std::env::temp_dir().join(format!("gut-review-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);

    for name in ["A", "B", "C"] {
        fs::write(path.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", name]);
    }

    git(&path, &["switch", "-c", "feature"]);
    for name in ["D", "E", "F"] {
        fs::write(path.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", name]);
    }

    path
}

fn gut(repo: &Path, args: &[&str]) -> std::process::Output {
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
    output
}

#[test]
fn review_json_describes_full_branch_against_merge_base() {
    let repo = repo();
    let merge_base = git(&repo, &["rev-parse", "main"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);

    let output = gut(&repo, &["review", "--base", "main", "--format", "json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");

    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["data"]["base"], merge_base);
    assert_eq!(value["data"]["mergeBase"], merge_base);
    assert_eq!(value["data"]["head"], head);
    assert_eq!(value["data"]["branch"], "feature");
    assert!(value["data"]["patch"].as_str().unwrap().contains("D.txt"));
    assert!(value["data"]["stat"].as_str().unwrap().contains("D.txt"));
    assert_eq!(
        value["data"]["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|commit| commit["subject"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["D", "E", "F"]
    );
    assert_eq!(
        value["data"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["D.txt", "E.txt", "F.txt"]
    );
    for file in value["data"]["files"].as_array().unwrap() {
        assert_eq!(file["additions"], 1);
        assert_eq!(file["deletions"], 0);
        let hunks = file["hunks"].as_array().expect("hunks");
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0]["header"].as_str().unwrap().starts_with("@@ "));
        assert_eq!(hunks[0]["oldStart"], 0);
        assert_eq!(hunks[0]["oldLines"], 0);
        assert_eq!(hunks[0]["newStart"], 1);
        assert_eq!(hunks[0]["newLines"], 1);
    }
}

#[test]
fn review_default_shows_full_branch_patch() {
    let repo = repo();
    let output = gut(&repo, &["review", "--base", "main"]);
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(stdout.contains("D.txt"));
    assert!(stdout.contains("E.txt"));
    assert!(stdout.contains("F.txt"));
}

#[test]
fn focused_review_views_render_from_the_same_model() {
    let repo = repo();

    let stat = String::from_utf8(gut(&repo, &["review", "--base", "main", "--stat"]).stdout)
        .expect("utf8 stat");
    assert!(stat.contains("D.txt"));
    assert!(stat.contains("3 files changed"));

    let commits = String::from_utf8(gut(&repo, &["review", "--base", "main", "--commits"]).stdout)
        .expect("utf8 commits");
    assert!(commits.contains(" D"));
    assert!(commits.contains(" E"));
    assert!(commits.contains(" F"));

    let files = String::from_utf8(gut(&repo, &["review", "--base", "main", "--files"]).stdout)
        .expect("utf8 files");
    assert!(files.contains("A\tD.txt"));
    assert!(files.contains("A\tE.txt"));
    assert!(files.contains("A\tF.txt"));
}
