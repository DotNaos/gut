# Editor integration contract

This document defines the stable `gut` engine contract for editor and UI clients such as `dotnaos/ui`.

## Transport and versioning

Start one persistent process in the repository:

```sh
gut runtime
```

The runtime uses newline-delimited JSON (NDJSON) over stdin/stdout. Each request is one JSON object on one line:

```json
{"id":"request-1","method":"runtime.describe","params":{}}
```

Each response echoes the request `id` and carries the protocol schema version:

```json
{"schemaVersion":1,"id":"request-1","result":{}}
```

Errors are returned without terminating the process:

```json
{"schemaVersion":1,"id":"request-1","error":{"message":"..."}}
```

Clients must use `runtime.describe` for capability discovery. `protocolVersion` is the compatibility boundary for request methods and event semantics. Structured result objects reuse the same field names as the corresponding `gut --json` models.

## Capability discovery

`runtime.describe` takes no parameters and returns:

- `protocolVersion`
- `transport`
- supported `methods`
- emitted `events`
- supported `commitPlacementModes`

The initial protocol version is `1`.

## Read workflows

### Repository state

Request:

```json
{"id":"repo","method":"repository.get"}
```

The result contains:

- `root`
- `branch`
- `head`
- `dirty`
- `worktree` with porcelain status lines

### Branch/status overview

Request:

```json
{"id":"status","method":"status.get","params":{"remote":"origin","main":"main","local":false}}
```

The result is the same status model used by `gut status --json`: branch inclusion/conflict state plus clean/dirty registered worktrees.

### Commit/history view

Request:

```json
{"id":"log","method":"log.get"}
```

The result contains current `branch`, `head`, and ordered commits. Each commit exposes its `id`, `parents`, and `subject`, which is sufficient to build the initial commit/history view without parsing terminal output.

### Branch-diff review

Request:

```json
{"id":"review","method":"review.get","params":{"base":"origin/main"}}
```

The result is the same model as `gut review --json`, including:

- branch/base/merge-base/HEAD
- commits
- changed files and status
- additions/deletions
- parsed diff hunks
- complete stat and patch text

## Repository state and optimistic concurrency

`repository.get` returns `branch`, `head`, `stateToken`, dirty state, and the porcelain worktree view. `stateToken` changes when the branch, HEAD, index, tracked working tree, or untracked file contents change.

Mutating requests accept optional `expectedState` and `expectedHead`. Editors should pass the values from the state they rendered:

```json
{
  "id":"place",
  "method":"commit.place",
  "params":{
    "mode":"update",
    "target":"<commit-id>",
    "expectedState":"<state-token>",
    "expectedHead":"<head>"
  }
}
```

If either expectation is stale, the runtime performs no mutation and returns a structured error:

```json
{
  "error":{
    "code":"stale_repository_state",
    "message":"stale repository state: ...",
    "data":{
      "expectedState":"...",
      "actualState":"...",
      "expectedHead":"...",
      "actualHead":"..."
    }
  }
}
```

`operation.undo` supports the same guards. Stored rewrite plans already contain the state token and HEAD and are always guarded when applied.

## Working changes and selection

Editors should use `changes.get` (or `gut changes --json`) instead of parsing `git diff`. The result separates staged and unstaged layers per path, marks untracked files, and exposes stable IDs for files and text hunks. Hunk IDs identify the exact layer/path/patch content and therefore change when that hunk changes.

```json
{"id":"changes","method":"changes.get"}
```

A complete path can be selected with `files`; selectable text hunks can be selected with `hunks`. Non-selected changes retain their original staged, unstaged, or untracked state. Additions, deletions, binary files, and untracked files are whole-file selections.

## Commit placement / drag and drop

A UI must map drag placement to `commit.place` without reproducing Git rewrite logic:

| UI intent | Runtime mode | Suggested UI wording |
| --- | --- | --- |
| Drop before a commit | `before` | Insert before |
| Drop onto a commit | `update` | Add to commit |
| Drop after a commit | `after` | Insert after |

Request shape:

```json
{
  "id":"place",
  "method":"commit.place",
  "params":{
    "mode":"before",
    "target":"<commit-id>",
    "message":"Optional new commit message",
    "files":["src/foo.rs"],
    "hunks":["h-0123456789abcdef"]
  }
}
```

`message` is valid for `before` and `after`; `update` preserves the target commit message. `files` and `hunks` are optional; omitting both preserves the original behavior of placing all current changes. The result includes `operationId`, `targetBefore`, `headBefore`, and `headAfter`.

Clients must treat the three mode identifiers as semantic API values. Display wording can be localized or changed independently.

Commit placement supports histories containing merge commits and preserves their topology. `target` must identify a non-merge ancestor of HEAD, including a commit on a merged side branch. A merge commit itself is rejected as an ambiguous placement target. Failed rewrites automatically return to the original HEAD and restore working-tree/index state before returning an error.

## Rewrite plans

Editors and agents can request a non-mutating plan before history editing:

```json
{"id":"plan","method":"commit.plan","params":{"mode":"update","target":"<commit-id>","hunks":["h-..."]}}
```

The plan returns a stable `id`, repository `stateToken`, branch/HEAD/target, affected commits with parent topology, whether merge topology must be preserved, and the exact file/hunk selection. Planning does not change HEAD, refs, index, working tree, or operation history; only the plan document is persisted under Git metadata.

Apply the exact plan with:

```json
{"id":"apply","method":"plan.apply","params":{"plan":"plan-..."}}
```

Application is rejected as stale if the branch or repository state changed since planning. Clients must request a new plan rather than silently recalculating the operation.

## Operation history and undo

### List operations

```json
{"id":"ops","method":"operation.log"}
```

Each operation includes its stable `id`, `parent`, `kind`, branch, optional target, before/after HEADs, protected before/after refs, and timestamp.

### Compare an operation

```json
{"id":"op-diff","method":"operation.diff","params":{"operation":"<operation-id>"}}
```

The result contains the operation record plus its exact before/after patch.

### Undo

```json
{"id":"undo","method":"operation.undo","params":{"operation":"<operation-id>"}}
```

Omitting `operation` restores the state before the latest operation on the current branch. Undo requires a clean worktree, creates a new operation record, and does not push or rewrite remotes.

## Events

With the default watcher enabled, the runtime can emit event lines between normal responses:

```json
{"schemaVersion":1,"event":"repository.changed","data":{}}
{"schemaVersion":1,"event":"workingTree.changed","data":{}}
{"schemaVersion":1,"event":"operation.completed","data":{}}
```

Clients must distinguish responses by `id` and events by `event`; they must not rely on a response being the next output line after its request.

- `repository.changed`: branch, HEAD, or repository identity changed.
- `workingTree.changed`: porcelain working-tree state changed.
- `operation.completed`: one or more new `gut` operation records were observed.

Use `gut runtime --watch-interval-ms 0` if a client only needs request/response behavior.

## Ownership boundary

`gut` owns Git inspection, history rewriting, operation history, undo, and protocol semantics. `dotnaos/ui` owns presentation, interaction design, localization, drag/drop affordances, and visual state. The UI must not implement a second history-rewrite path with raw `fixup`, autosquash, or rebase commands.
