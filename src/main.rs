use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    process::{Command, ExitCode},
};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use serde::Serialize;

mod commit;
mod log;
mod operation;
mod repository;
mod review;
mod runtime;

#[derive(Parser)]
#[command(name = "gut", version, about = "Good Git: small Git helpers")]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    format: OutputFormat,

    /// Shortcut for --format json.
    #[arg(long, global = true, conflicts_with = "format")]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Human,
    Plain,
    Json,
    Values,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StatusFilter {
    AlreadyInMain,
    WouldChangeMain,
    Conflicts,
    CleanWorktrees,
    DirtyWorktrees,
}

#[derive(Subcommand)]
enum Commands {
    #[command(name = "__sequence-editor", hide = true)]
    SequenceEditor { todo: PathBuf },

    /// Place the current changes before, into, or after an existing commit.
    Commit {
        #[arg(long, conflicts_with_all = ["before", "after"], value_name = "COMMIT")]
        update: Option<String>,

        #[arg(long, conflicts_with_all = ["update", "after"], value_name = "COMMIT")]
        before: Option<String>,

        #[arg(long, conflicts_with_all = ["update", "before"], value_name = "COMMIT")]
        after: Option<String>,

        #[arg(short = 'm', long)]
        message: Option<String>,
    },

    /// Show branch inclusion state and worktree cleanliness.
    Status {
        #[arg(long, default_value = "origin")]
        remote: String,

        #[arg(long, default_value = "main")]
        main: String,

        /// Compare local branches instead of remote branches.
        #[arg(long)]
        local: bool,

        /// Only show selected status categories. Repeat or comma-separate values.
        #[arg(long, value_enum, value_delimiter = ',')]
        filter: Vec<StatusFilter>,
    },

    /// List branches, excluding main.
    Branches {
        #[arg(long, default_value = "origin")]
        remote: String,

        #[arg(long, default_value = "main")]
        main: String,

        /// List local branches instead of remote branches.
        #[arg(long)]
        local: bool,
    },

    /// Review the current branch against its base.
    Review {
        #[arg(long, default_value = "origin/main")]
        base: String,

        #[arg(long, conflicts_with_all = ["commits", "files"])]
        stat: bool,

        #[arg(long, conflicts_with_all = ["stat", "files"])]
        commits: bool,

        #[arg(long, conflicts_with_all = ["stat", "commits"])]
        files: bool,
    },

    /// Show commit history for the current branch.
    Log,

    /// Inspect or restore gut operation history.
    Op {
        #[command(subcommand)]
        command: OpCommands,
    },

    /// Diff a remote branch against the merge-base with main.
    Diff {
        branch: String,

        #[arg(long, default_value = "origin")]
        remote: String,

        #[arg(long, default_value = "main")]
        main: String,
    },

    /// Show registered worktrees as clean or dirty.
    Worktrees,

    /// Run the persistent editor-facing stdio runtime.
    Runtime {
        /// Poll interval for repository change events. Use 0 to disable events.
        #[arg(long, default_value_t = 200)]
        watch_interval_ms: u64,
    },

    /// Upgrade gut from source using the install script.
    Upgrade,

    /// Generate shell completions.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum OpCommands {
    /// List recorded mutating gut operations, newest first.
    Log,

    /// Show the repository diff produced by an operation.
    Diff { operation: String },

    /// Restore the state from before an operation. Defaults to the latest operation on this branch.
    Undo { operation: Option<String> },
}

#[derive(Clone, Serialize)]
struct BranchStatus {
    already_in_main: Vec<String>,
    would_change_main: Vec<String>,
    conflicts: Vec<String>,
}

#[derive(Clone, Serialize)]
struct WorktreeStatus {
    clean: Vec<String>,
    dirty: Vec<String>,
}

#[derive(Clone, Serialize)]
struct StatusOutput {
    branches: BranchStatus,
    worktrees: WorktreeStatus,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonEnvelope<T> {
    schema_version: u32,
    data: T,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("gut: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let cli = Cli::parse();
    let format = if cli.json {
        OutputFormat::Json
    } else {
        cli.format
    };

    match cli.command {
        Commands::SequenceEditor { todo } => {
            commit::edit_rebase_todo(&todo)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Commit {
            update,
            before,
            after,
            message,
        } => {
            let placement = commit::Placement::from_args(update, before, after)?;
            let output = commit::place_current_changes(
                placement,
                message.as_deref(),
                matches!(format, OutputFormat::Json),
            )?;
            if matches!(format, OutputFormat::Json) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&JsonEnvelope {
                        schema_version: 1,
                        data: output,
                    })
                    .map_err(|error| error.to_string())?
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Status {
            remote,
            main,
            local,
            filter,
        } => {
            let output = StatusOutput {
                branches: branch_status(&remote, &main, local)?,
                worktrees: worktree_status()?,
            };
            print_status(format, &output, &filter)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Branches {
            remote,
            main,
            local,
        } => {
            let branches = branches(&remote, &main, local)?;
            print_list(format, "BRANCHES", "branches", &branches)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Review {
            base,
            stat,
            commits,
            files,
        } => {
            let output = review::inspect(&base)?;
            if matches!(format, OutputFormat::Json) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&JsonEnvelope {
                        schema_version: 1,
                        data: output,
                    })
                    .map_err(|error| error.to_string())?
                );
            } else if stat {
                print!("{}", output.stat);
            } else if commits {
                for commit in &output.commits {
                    let short = &commit.id[..commit.id.len().min(12)];
                    println!("{short} {}", commit.subject);
                }
            } else if files {
                for file in &output.files {
                    println!("{}\t{}", file.status, file.path);
                }
            } else {
                print!("{}", output.patch);
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Log => {
            let output = log::inspect()?;
            if matches!(format, OutputFormat::Json) {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&JsonEnvelope {
                        schema_version: 1,
                        data: output,
                    })
                    .map_err(|error| error.to_string())?
                );
            } else {
                for commit in &output.commits {
                    match format {
                        OutputFormat::Human => {
                            let short = &commit.id[..commit.id.len().min(12)];
                            println!("{short} {}", commit.subject);
                        }
                        OutputFormat::Plain => println!("{}\t{}", commit.id, commit.subject),
                        OutputFormat::Values => println!("{}", commit.id),
                        OutputFormat::Json => unreachable!(),
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Op { command } => {
            match command {
                OpCommands::Log => {
                    let records = operation::log()?;
                    if matches!(format, OutputFormat::Json) {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&JsonEnvelope {
                                schema_version: 1,
                                data: records,
                            })
                            .map_err(|error| error.to_string())?
                        );
                    } else {
                        for record in records {
                            match format {
                                OutputFormat::Human => {
                                    let target = record
                                        .target
                                        .as_deref()
                                        .map(|target| format!(" target={target}"))
                                        .unwrap_or_default();
                                    println!(
                                        "{}  {}  {}  {} -> {}{}",
                                        record.id,
                                        record.kind.as_str(),
                                        record.branch,
                                        record.head_before,
                                        record.head_after,
                                        target
                                    );
                                }
                                OutputFormat::Plain => println!(
                                    "{}\t{}\t{}\t{}\t{}\t{}",
                                    record.id,
                                    record.kind.as_str(),
                                    record.branch,
                                    record.head_before,
                                    record.head_after,
                                    record.target.as_deref().unwrap_or("")
                                ),
                                OutputFormat::Values => println!("{}", record.id),
                                OutputFormat::Json => unreachable!(),
                            }
                        }
                    }
                }
                OpCommands::Diff { operation: id } => {
                    if matches!(format, OutputFormat::Json) {
                        let output = operation::diff(&id)?;
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&JsonEnvelope {
                                schema_version: 1,
                                data: output,
                            })
                            .map_err(|error| error.to_string())?
                        );
                    } else {
                        operation::show_diff(&id)?;
                    }
                }
                OpCommands::Undo { operation: id } => {
                    let output = operation::undo(id.as_deref())?;
                    if matches!(format, OutputFormat::Json) {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&JsonEnvelope {
                                schema_version: 1,
                                data: output,
                            })
                            .map_err(|error| error.to_string())?
                        );
                    } else {
                        match format {
                            OutputFormat::Human => println!(
                                "restored {} -> {} (operation {})",
                                output.head_before, output.head_after, output.id
                            ),
                            OutputFormat::Plain => println!(
                                "{}\t{}\t{}",
                                output.id, output.head_before, output.head_after
                            ),
                            OutputFormat::Values => println!("{}", output.id),
                            OutputFormat::Json => unreachable!(),
                        }
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Diff {
            branch,
            remote,
            main,
        } => {
            let main_ref = format!("{remote}/{main}");
            let branch_ref = normalize_remote_branch(&branch, &remote);
            let status = Command::new("git")
                .args(["diff", &format!("{main_ref}...{branch_ref}")])
                .status()
                .map_err(|error| format!("failed to run git diff: {error}"))?;

            Ok(if status.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Commands::Worktrees => {
            let status = worktree_status()?;
            print_worktrees(format, &status)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Runtime { watch_interval_ms } => {
            runtime::run(watch_interval_ms)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Upgrade => upgrade(),
        Commands::Completions { shell } => {
            let mut command = Cli::command();
            let name = command.get_name().to_owned();
            generate(shell, &mut command, name, &mut io::stdout());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn build_commit() -> &'static str {
    option_env!("GUT_BUILD_COMMIT").unwrap_or("unknown")
}

fn upgrade() -> Result<ExitCode, String> {
    let script = r#"
set -eu
commit="$(git ls-remote https://github.com/DotNaos/gut.git refs/heads/main | awk '{print $1}')"
[ -n "$commit" ]
curl -fsSL "https://raw.githubusercontent.com/DotNaos/gut/$commit/install.sh" | sh
"#;

    let status = Command::new("sh")
        .args(["-c", script])
        .env("GUT_UPGRADE_FROM", build_commit())
        .env("GUT_UPGRADE_FROM_VERSION", env!("CARGO_PKG_VERSION"))
        .status()
        .map_err(|error| format!("failed to run upgrade: {error}"))?;

    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn branches(remote: &str, main: &str, local: bool) -> Result<Vec<String>, String> {
    if local {
        let output = git_output(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"])?;

        return Ok(output
            .lines()
            .filter(|branch| *branch != main)
            .map(str::to_owned)
            .collect());
    }

    remote_branches(remote, main)
}

fn remote_branches(remote: &str, main: &str) -> Result<Vec<String>, String> {
    let refs = format!("refs/remotes/{remote}/");
    let output = git_output(&["for-each-ref", "--format=%(refname:short)", &refs])?;
    let main_ref = format!("{remote}/{main}");
    let head_ref = format!("{remote}/HEAD");

    Ok(output
        .lines()
        .filter(|branch| *branch != main_ref && *branch != head_ref)
        .map(str::to_owned)
        .collect())
}

fn normalize_remote_branch(branch: &str, remote: &str) -> String {
    let prefix = format!("{remote}/");
    if branch.starts_with(&prefix) {
        branch.to_owned()
    } else {
        format!("{remote}/{branch}")
    }
}

fn branch_status(remote: &str, main: &str, local: bool) -> Result<BranchStatus, String> {
    let branches = branches(remote, main, local)?;
    let main_ref = if local {
        main.to_owned()
    } else {
        format!("{remote}/{main}")
    };
    let main_tree_output = git_output(&["rev-parse", &format!("{main_ref}^{{tree}}")])?;
    let main_tree = main_tree_output.trim();

    let mut status = BranchStatus {
        already_in_main: Vec::new(),
        would_change_main: Vec::new(),
        conflicts: Vec::new(),
    };

    for branch in branches {
        let output = Command::new("git")
            .args(["merge-tree", "--write-tree", &main_ref, &branch])
            .output()
            .map_err(|error| format!("failed to run git merge-tree: {error}"))?;

        if !output.status.success() {
            status.conflicts.push(branch);
            continue;
        }

        let stdout = String::from_utf8(output.stdout)
            .map_err(|_| "git merge-tree returned non-UTF-8 output".to_owned())?;
        let merged_tree = stdout.lines().next().unwrap_or_default().trim();

        if merged_tree == main_tree {
            status.already_in_main.push(branch);
        } else {
            status.would_change_main.push(branch);
        }
    }

    Ok(status)
}

fn worktree_status() -> Result<WorktreeStatus, String> {
    let output = git_output(&["worktree", "list", "--porcelain"])?;
    let paths: Vec<PathBuf> = output
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect();

    let mut status = WorktreeStatus {
        clean: Vec::new(),
        dirty: Vec::new(),
    };

    for path in paths {
        let path_string = path.to_string_lossy().into_owned();
        let output = Command::new("git")
            .args(["-C", &path_string, "status", "--porcelain"])
            .output()
            .map_err(|error| format!("failed to inspect {path_string}: {error}"))?;

        if !output.status.success() {
            return Err(format!("git status failed for {path_string}"));
        }

        if output.stdout.is_empty() {
            status.clean.push(path_string);
        } else {
            status.dirty.push(path_string);
        }
    }

    Ok(status)
}

fn git_output(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_owned());
    }

    String::from_utf8(output.stdout).map_err(|_| "git returned non-UTF-8 output".to_owned())
}

fn filter_selected(filters: &[StatusFilter], filter: StatusFilter) -> bool {
    filters.is_empty() || filters.contains(&filter)
}

fn filtered_status(output: &StatusOutput, filters: &[StatusFilter]) -> StatusOutput {
    let mut filtered = output.clone();

    if !filter_selected(filters, StatusFilter::AlreadyInMain) {
        filtered.branches.already_in_main.clear();
    }
    if !filter_selected(filters, StatusFilter::WouldChangeMain) {
        filtered.branches.would_change_main.clear();
    }
    if !filter_selected(filters, StatusFilter::Conflicts) {
        filtered.branches.conflicts.clear();
    }
    if !filter_selected(filters, StatusFilter::CleanWorktrees) {
        filtered.worktrees.clean.clear();
    }
    if !filter_selected(filters, StatusFilter::DirtyWorktrees) {
        filtered.worktrees.dirty.clear();
    }

    filtered
}

fn print_status(
    format: OutputFormat,
    output: &StatusOutput,
    filters: &[StatusFilter],
) -> Result<(), String> {
    match format {
        OutputFormat::Human => {
            let sections = [
                (
                    StatusFilter::AlreadyInMain,
                    "ALREADY IN MAIN",
                    &output.branches.already_in_main,
                ),
                (
                    StatusFilter::WouldChangeMain,
                    "WOULD CHANGE MAIN",
                    &output.branches.would_change_main,
                ),
                (
                    StatusFilter::Conflicts,
                    "CONFLICTS",
                    &output.branches.conflicts,
                ),
                (
                    StatusFilter::CleanWorktrees,
                    "CLEAN WORKTREES",
                    &output.worktrees.clean,
                ),
                (
                    StatusFilter::DirtyWorktrees,
                    "DIRTY WORKTREES",
                    &output.worktrees.dirty,
                ),
            ];

            let mut printed = false;
            for (filter, title, values) in sections {
                if filter_selected(filters, filter) {
                    if printed {
                        println!();
                    }
                    print_section(title, values);
                    printed = true;
                }
            }
        }
        OutputFormat::Plain => {
            if filter_selected(filters, StatusFilter::AlreadyInMain) {
                for branch in &output.branches.already_in_main {
                    println!("already-in-main\t{branch}");
                }
            }
            if filter_selected(filters, StatusFilter::WouldChangeMain) {
                for branch in &output.branches.would_change_main {
                    println!("would-change-main\t{branch}");
                }
            }
            if filter_selected(filters, StatusFilter::Conflicts) {
                for branch in &output.branches.conflicts {
                    println!("conflict\t{branch}");
                }
            }
            if filter_selected(filters, StatusFilter::CleanWorktrees) {
                for path in &output.worktrees.clean {
                    println!("clean\t{path}");
                }
            }
            if filter_selected(filters, StatusFilter::DirtyWorktrees) {
                for path in &output.worktrees.dirty {
                    println!("dirty\t{path}");
                }
            }
        }
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&JsonEnvelope {
                schema_version: 1,
                data: filtered_status(output, filters),
            })
            .map_err(|error| error.to_string())?
        ),
        OutputFormat::Values => {
            if filters.is_empty() {
                return Err("--format values requires at least one --filter".to_owned());
            }

            if filter_selected(filters, StatusFilter::AlreadyInMain) {
                print_values(&output.branches.already_in_main);
            }
            if filter_selected(filters, StatusFilter::WouldChangeMain) {
                print_values(&output.branches.would_change_main);
            }
            if filter_selected(filters, StatusFilter::Conflicts) {
                print_values(&output.branches.conflicts);
            }
            if filter_selected(filters, StatusFilter::CleanWorktrees) {
                print_values(&output.worktrees.clean);
            }
            if filter_selected(filters, StatusFilter::DirtyWorktrees) {
                print_values(&output.worktrees.dirty);
            }
        }
    }

    Ok(())
}

fn print_worktrees(format: OutputFormat, status: &WorktreeStatus) -> Result<(), String> {
    match format {
        OutputFormat::Human => {
            print_section("CLEAN WORKTREES", &status.clean);
            println!();
            print_section("DIRTY WORKTREES", &status.dirty);
        }
        OutputFormat::Plain => {
            for path in &status.clean {
                println!("clean\t{path}");
            }
            for path in &status.dirty {
                println!("dirty\t{path}");
            }
        }
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&JsonEnvelope {
                schema_version: 1,
                data: status,
            })
            .map_err(|error| error.to_string())?
        ),
        OutputFormat::Values => {
            print_values(&status.clean);
            print_values(&status.dirty);
        }
    }

    Ok(())
}

fn print_list(
    format: OutputFormat,
    human_title: &str,
    json_key: &str,
    values: &[String],
) -> Result<(), String> {
    match format {
        OutputFormat::Human => print_section(human_title, values),
        OutputFormat::Plain | OutputFormat::Values => print_values(values),
        OutputFormat::Json => {
            let mut object = BTreeMap::new();
            object.insert(json_key, values);
            println!(
                "{}",
                serde_json::to_string_pretty(&JsonEnvelope {
                    schema_version: 1,
                    data: object,
                })
                .map_err(|error| error.to_string())?
            );
        }
    }

    Ok(())
}

fn print_values(values: &[String]) {
    for value in values {
        println!("{value}");
    }
}

fn print_section(title: &str, values: &[String]) {
    println!("=== {title} ===");
    print_values(values);
}
