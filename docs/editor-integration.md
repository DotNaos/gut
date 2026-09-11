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
    "message":"Optional new commit message"
  }
}
```

`message` is valid for `before` and `after`; `update` preserves the target commit message. The result includes `operationId`, `targetBefore`, `headBefore`, and `headAfter`.

Clients must treat the three mode identifiers as semantic API values. Display wording can be localized or changed independently.

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
