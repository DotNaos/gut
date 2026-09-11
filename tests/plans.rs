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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn repo() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gut-plan-{}-{unique}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "gut tests"]);
    git(&path, &["config", "user.email", "gut@example.invalid"]);
    let content = (1..=24)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    fs::write(path.join("work.txt"), content).unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "A"]);
    for name in ["B", "C", "D"] {
        fs::write(path.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        git(&path, &["add", "."]);
        git(&path, &["commit", "-m", name]);
    }
    path
}

fn merge_repo() -> PathBuf {
    let path = repo();
    let b = commit(&path, "B");
    git(&path, &["switch", "-c", "side", &b]);
    fs::write(path.join("side.txt"), "side\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "Side"]);
    git(&path, &["switch", "main"]);
    git(&path, &["merge", "--no-ff", "side", "-m", "Merge side"]);
    fs::write(path.join("after-merge.txt"), "after\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "After merge"]);
    path
}

fn commit(repo: &Path, subject: &str) -> String {
    git(repo, &["log", "--format=%H%x09%s", "--all"])
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

fn replace_line(repo: &Path, line: usize, replacement: &str) {
    let path = repo.join("work.txt");
    let mut lines = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines[line - 1] = replacement.to_owned();
    fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
}

#[test]
fn plan_is_git_state_read_only_and_apply_executes_exact_plan() {
    let repo = repo();
    let b = commit(&repo, "B");
    replace_line(&repo, 2, "selected");
    replace_line(&repo, 21, "remaining");
    let changes = gut_json(&repo, &["changes", "--json"]);
    let hunk = changes["data"]["files"][0]["unstaged"]["hunks"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let head_before = git(&repo, &["rev-parse", "HEAD"]);
    let status_before = git(&repo, &["status", "--porcelain=v1"]);
    let refs_before = git(
        &repo,
        &["for-each-ref", "--format=%(refname)%00%(objectname)"],
    );
    let plan = gut_json(
        &repo,
        &[
            "commit", "--update", &b, "--hunk", &hunk, "--plan", "--json",
        ],
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(git(&repo, &["status", "--porcelain=v1"]), status_before);
    assert_eq!(
        git(
            &repo,
            &["for-each-ref", "--format=%(refname)%00%(objectname)"]
        ),
        refs_before
    );
    assert_eq!(plan["data"]["operation"], "update");
    assert_eq!(plan["data"]["targetBefore"], b);
    assert_eq!(plan["data"]["preserveMerges"], false);
    assert_eq!(plan["data"]["selection"]["hunks"][0], hunk);
    assert_eq!(
        plan["data"]["affectedCommits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["subject"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["B", "C", "D"]
    );
    let id = plan["data"]["id"].as_str().unwrap();
    let applied = gut_json(&repo, &["apply", id, "--json"]);
    assert_eq!(applied["data"]["operation"], "update");
    assert_ne!(applied["data"]["headAfter"], head_before);
    let remaining = git(&repo, &["diff", "--", "work.txt"]);
    assert!(!remaining.contains("selected"));
    assert!(remaining.contains("remaining"));
}

#[test]
fn stale_plan_is_rejected_before_mutation() {
    let repo = repo();
    let b = commit(&repo, "B");
    replace_line(&repo, 2, "planned-change");
    let plan = gut_json(&repo, &["commit", "--update", &b, "--plan", "--json"]);
    let id = plan["data"]["id"].as_str().unwrap();
    let head_before = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("new-after-plan.txt"), "new state\n").unwrap();
    let output = gut_output(&repo, &["apply", id, "--json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("stale plan"), "stderr: {stderr}");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head_before);
    assert!(repo.join("new-after-plan.txt").exists());
    assert!(
        fs::read_to_string(repo.join("work.txt"))
            .unwrap()
            .contains("planned-change")
    );
}

#[test]
fn merge_plan_reports_topology_that_will_be_preserved() {
    let repo = merge_repo();
    let b = commit(&repo, "B");
    replace_line(&repo, 2, "merge-plan-change");
    let head_before = git(&repo, &["rev-parse", "HEAD"]);
    let plan = gut_json(&repo, &["commit", "--update", &b, "--plan", "--json"]);
    assert_eq!(plan["data"]["preserveMerges"], true);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head_before);
    let commits = plan["data"]["affectedCommits"].as_array().unwrap();
    assert!(
        commits
            .iter()
            .any(|c| c["parents"].as_array().unwrap().len() == 2),
        "plan did not expose merge topology: {commits:#?}"
    );
}

#[test]
fn runtime_can_plan_then_apply_in_the_same_process() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    let repo = repo();
    let b = commit(&repo, "B");
    replace_line(&repo, 2, "runtime-planned");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gut"))
        .current_dir(&repo)
        .args(["runtime", "--watch-interval-ms", "0"])
        .env("GIT_EDITOR", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn runtime");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    serde_json::to_writer(
        &mut stdin,
        &serde_json::json!({
            "id":"plan",
            "method":"commit.plan",
            "params":{"mode":"update","target":b}
        }),
    )
    .unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let planned: serde_json::Value = serde_json::from_str(&line).unwrap();
    let id = planned["result"]["id"].as_str().unwrap().to_owned();

    serde_json::to_writer(
        &mut stdin,
        &serde_json::json!({"id":"apply","method":"plan.apply","params":{"plan":id}}),
    )
    .unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let applied: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(applied["result"]["operation"], "update");

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}
