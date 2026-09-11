# gut

Good Git: small Git helpers for everyday repository inspection and cleanup.

## Install

Builds from source and installs to `~/.local/bin/gut`:

```sh
curl -fsSL https://raw.githubusercontent.com/DotNaos/gut/main/install.sh | sh
```

Requires `git` and Rust/Cargo.

## Commands

```sh
gut commit --update <commit>
gut commit --before <commit> -m "message"
gut commit --after <commit> -m "message"
gut status
gut review
gut op log
gut op diff <operation>
gut op undo [operation]
gut branches
gut diff <branch>
gut worktrees
gut completions <shell>
```

`gut commit` places all current changes at a specific point in linear local history. `--update` adds them to an existing commit; `--before` and `--after` create a new commit around the selected commit and rewrite descendants automatically.

`gut review` shows the full current-branch diff against the merge-base with `origin/main`. Use `--base <ref>` for another base, or `--stat`, `--commits`, and `--files` for focused human-readable views. With `--format json`, it returns the resolved base, merge-base, HEAD, commits, and changed files as structured data.

History-editing operations are recorded locally by `gut`. `gut op log` lists them, `gut op diff <operation>` shows the exact before/after diff, and `gut op undo [operation]` restores the state from before an operation. Undo requires a clean working tree and never pushes or rewrites a remote. Gut keeps before/after Git refs for recorded operations so rewritten commits remain reachable locally.

`gut status` is the combined overview: it checks whether each remote branch would still change `origin/main`, and whether registered worktrees are clean or dirty.

`gut diff foo` runs the equivalent of:

```sh
git diff origin/main...origin/foo
```

The branch inclusion check is tree/content based. It can therefore recognize changes that are already present in `main` after a squash merge even when the original commits are not ancestors of `main`.

Use `--remote` and `--main` when the defaults are different:

```sh
gut status --remote upstream --main trunk
```

## Output formats

Human-readable output is the default:

```text
=== ALREADY IN MAIN ===
origin/old-feature

=== WOULD CHANGE MAIN ===
origin/current-feature

=== CONFLICTS ===
origin/conflicting-feature

=== CLEAN WORKTREES ===
/home/oli/projects/example

=== DIRTY WORKTREES ===
/home/oli/projects/example.worktrees/wip
```

For scripting:

```sh
gut status --format plain
gut status --format json
```

Commit placement also has structured JSON output:

```sh
gut commit --update <commit> --format json
```

```json
{
  "schemaVersion": 1,
  "data": {
    "operation": "update",
    "targetBefore": "<commit>",
    "headBefore": "<commit>",
    "headAfter": "<commit>",
    "operationId": "op-..."
  }
}
```

Operation history also uses the same versioned JSON envelope:

```sh
gut op log --format json
gut op diff <operation> --format json
gut op undo <operation> --format json
```

## Shell completion

Generate completions directly:

```sh
gut completions bash
gut completions zsh
gut completions fish
gut completions powershell
```

The installer writes completions automatically for Bash, Zsh, and Fish when it can identify the current shell. For Zsh, `~/.zfunc` must be present in `fpath`.
