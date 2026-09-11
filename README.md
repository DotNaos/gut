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
gut log
gut op log
gut op diff <operation>
gut op undo [operation]
gut branches
gut diff <branch>
gut worktrees
gut runtime
gut completions <shell>
```

`gut commit` places all current changes at a specific point in local history. `--update` adds them to an existing commit; `--before` and `--after` create a new commit around the selected commit and rewrite descendants automatically. Histories containing merges are rewritten with merge topology preserved. The target itself must be a non-merge commit; placement directly relative to a merge commit is rejected as ambiguous.

If a history rewrite conflicts or otherwise fails, `gut` aborts the rewrite and restores the original HEAD, working-tree contents, and Git index. The protected before-state ref is retained if automatic rollback itself cannot complete.

`gut review` shows the full current-branch diff against the merge-base with `origin/main`. Use `--base <ref>` for another base, or `--stat`, `--commits`, and `--files` for focused human-readable views. JSON review output includes branch/base/merge-base/HEAD information, commits, changed files, additions/deletions, parsed diff hunks, stat output, and the complete patch.

`gut log` shows the current commit history. Its structured form includes the current branch, HEAD, commit IDs, parent IDs, and subjects.

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

`--json` is a global shortcut for `--format json`, so editor-facing calls can use:

```sh
gut status --json
gut log --json
gut review --json
gut op log --json
```

All JSON-capable commands use a versioned top-level envelope with `schemaVersion` and `data`. Existing command-specific field names remain stable inside `data`.

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

## Editor runtime

`gut runtime` keeps one local process alive for editor and UI integrations. It uses newline-delimited JSON over stdin/stdout and calls the same core operations as the CLI.

Example request and response:

```json
{"id":"review-1","method":"review.get","params":{"base":"main"}}
{"schemaVersion":1,"id":"review-1","result":{"branch":"feature"}}
```

Supported query methods:

- `repository.get`
- `status.get`
- `log.get`
- `review.get`
- `operation.log`
- `operation.diff`

Supported mutation methods:

- `commit.place` with `mode` = `before`, `update`, or `after`
- `operation.undo`

The runtime also emits versioned event lines when observed state changes:

```json
{"schemaVersion":1,"event":"repository.changed","data":{}}
{"schemaVersion":1,"event":"workingTree.changed","data":{}}
{"schemaVersion":1,"event":"operation.completed","data":{}}
```

The default repository polling interval is 200 ms. Use `gut runtime --watch-interval-ms 0` to disable events and keep request/response handling only. Protocol errors are returned as response objects and do not terminate the runtime.

The stable editor/UI contract, including drag-and-drop placement semantics and ownership boundaries, is documented in [`docs/editor-integration.md`](docs/editor-integration.md). Clients can call `runtime.describe` to discover the protocol version, methods, events, and supported commit-placement modes.

## Shell completion

Generate completions directly:

```sh
gut completions bash
gut completions zsh
gut completions fish
gut completions powershell
```

The installer writes completions automatically for Bash, Zsh, and Fish when it can identify the current shell. For Zsh, `~/.zfunc` must be present in `fpath`.
