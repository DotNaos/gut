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
gut status
gut branches
gut diff <branch>
gut worktrees
gut completions <shell>
```

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

## Shell completion

Generate completions directly:

```sh
gut completions bash
gut completions zsh
gut completions fish
gut completions powershell
```

The installer writes completions automatically for Bash, Zsh, and Fish when it can identify the current shell. For Zsh, `~/.zfunc` must be present in `fpath`.
