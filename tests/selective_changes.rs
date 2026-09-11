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

fn git_success(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git status command")
        .success()
}

fn repo() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "gut-selective-changes-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);

    for name in [
        "selected.txt",
        "hunks.txt",
        "remain-staged.txt",
        "remain-unstaged.txt",
    ] {
        let content = (1..=24)
            .map(|line| format!("{name}-line-{line}\n"))
            .collect::<String>();
        fs::write(path.join(name), content).unwrap();
    }
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
            let (hash, found_subject) = line.split_once('\t')?;
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

fn gut(repo: &Path, args: &[&str]) -> std::process::Output {
    let output = gut_output(repo, args);
    assert!(
        output.status.success(),
        "gut {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn changes(repo: &Path) -> serde_json::Value {
    let output = gut(repo, &["changes", "--json"]);
    serde_json::from_slice(&output.stdout).expect("valid changes json")
}

fn file<'a>(value: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    value["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("missing {path}"))
}

fn replace_line(repo: &Path, path: &str, line: usize, replacement: &str) {
    let full = repo.join(path);
    let mut lines = fs::read_to_string(&full)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines[line - 1] = replacement.to_owned();
    fs::write(full, format!("{}\n", lines.join("\n"))).unwrap();
}

#[test]
fn changes_json_separates_layers_and_has_stable_hunk_ids() {
    let repo = repo();
    replace_line(&repo, "remain-staged.txt", 2, "staged-change");
    git(&repo, &["add", "remain-staged.txt"]);
    replace_line(&repo, "remain-unstaged.txt", 3, "unstaged-change");
    replace_line(&repo, "hunks.txt", 2, "hunk-one");
    replace_line(&repo, "hunks.txt", 21, "hunk-two");
    fs::write(repo.join("new.txt"), "untracked\n").unwrap();

    let first = changes(&repo);
    let second = changes(&repo);
    assert_eq!(first["schemaVersion"], 1);
    assert!(file(&first, "remain-staged.txt")["staged"].is_object());
    assert!(file(&first, "remain-staged.txt")["unstaged"].is_null());
    assert!(file(&first, "remain-unstaged.txt")["unstaged"].is_object());
    assert_eq!(file(&first, "new.txt")["untracked"], true);

    let first_hunks = file(&first, "hunks.txt")["unstaged"]["hunks"]
        .as_array()
        .unwrap();
    let second_hunks = file(&second, "hunks.txt")["unstaged"]["hunks"]
        .as_array()
        .unwrap();
    assert_eq!(first_hunks.len(), 2);
    assert!(first_hunks.iter().all(|hunk| hunk["selectable"] == true));
    assert_eq!(first_hunks[0]["id"], second_hunks[0]["id"]);
    assert_eq!(first_hunks[1]["id"], second_hunks[1]["id"]);
    assert_ne!(first_hunks[0]["id"], first_hunks[1]["id"]);
}

#[test]
fn selected_hunk_moves_into_history_and_leaves_other_hunk_unstaged() {
    let repo = repo();
    let old_b = commit(&repo, "B");
    replace_line(&repo, "hunks.txt", 2, "selected-hunk");
    replace_line(&repo, "hunks.txt", 21, "remaining-hunk");

    let state = changes(&repo);
    let hunks = file(&state, "hunks.txt")["unstaged"]["hunks"]
        .as_array()
        .unwrap();
    let selected = hunks[0]["id"].as_str().unwrap();
    gut(&repo, &["commit", "--update", &old_b, "--hunk", selected]);

    let new_b = commit(&repo, "B");
    let committed = git(&repo, &["show", &format!("{new_b}:hunks.txt")]);
    assert!(committed.contains("selected-hunk"));
    assert!(committed.contains("hunks.txt-line-21"));

    assert!(git_success(
        &repo,
        &["diff", "--cached", "--quiet", "--", "hunks.txt"]
    ));
    assert!(!git_success(&repo, &["diff", "--quiet", "--", "hunks.txt"]));
    let remaining = git(&repo, &["diff", "--", "hunks.txt"]);
    assert!(!remaining.contains("selected-hunk"));
    assert!(remaining.contains("remaining-hunk"));
}

#[test]
fn selected_file_can_include_staged_and_unstaged_parts_without_disturbing_other_state() {
    let repo = repo();
    let old_b = commit(&repo, "B");

    replace_line(&repo, "selected.txt", 2, "selected-staged");
    git(&repo, &["add", "selected.txt"]);
    replace_line(&repo, "selected.txt", 21, "selected-unstaged");

    replace_line(&repo, "remain-staged.txt", 4, "remain-staged");
    git(&repo, &["add", "remain-staged.txt"]);
    replace_line(&repo, "remain-unstaged.txt", 5, "remain-unstaged");
    fs::write(repo.join("remain-untracked.txt"), "remain-untracked\n").unwrap();

    gut(
        &repo,
        &["commit", "--update", &old_b, "--file", "selected.txt"],
    );

    assert_eq!(
        git(&repo, &["status", "--short"]),
        "M  remain-staged.txt\n M remain-unstaged.txt\n?? remain-untracked.txt"
    );
    let head_file = git(&repo, &["show", "HEAD:selected.txt"]);
    assert!(head_file.contains("selected-staged"));
    assert!(head_file.contains("selected-unstaged"));
}

#[test]
fn selected_untracked_file_becomes_history_while_other_changes_remain() {
    let repo = repo();
    let old_b = commit(&repo, "B");
    replace_line(&repo, "remain-staged.txt", 4, "remain-staged");
    git(&repo, &["add", "remain-staged.txt"]);
    replace_line(&repo, "remain-unstaged.txt", 5, "remain-unstaged");
    fs::write(repo.join("selected-new.txt"), "selected-new\n").unwrap();
    fs::write(repo.join("remain-new.txt"), "remain-new\n").unwrap();

    gut(
        &repo,
        &["commit", "--update", &old_b, "--file", "selected-new.txt"],
    );

    assert_eq!(
        git(&repo, &["show", "HEAD:selected-new.txt"]),
        "selected-new"
    );
    assert_eq!(
        git(&repo, &["status", "--short"]),
        "M  remain-staged.txt\n M remain-unstaged.txt\n?? remain-new.txt"
    );
}
