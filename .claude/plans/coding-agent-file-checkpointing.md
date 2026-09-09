# Spec: File checkpointing and workspace undo

> Report item **9** (Tier 3). The correctness bug in this set — `/undo` currently lies.
> Prerequisite for granting broad approval rules safely.

## Context

`/undo` trims the **transcript**. `transcript::undo_turns`
([`transcript.rs:72`](../../crates/aictl-core/src/transcript.rs)) walks back to the previous
user prompt and truncates `messages`. The files on disk stay edited.

So after `/undo`, the conversation says the edit never happened and the working tree says it
did. The model's next turn reads a file whose contents contradict the transcript it was given,
and the user believes they reverted something they did not. That is not a missing feature —
it is a silent inconsistency between two things the user reasonably assumes are in sync.

`coding.rs` already tracks what was touched: `record_workspace_change`
([`coding.rs:502`](../../crates/aictl-core/src/coding.rs)) is called after every `write_file` /
`edit_file` / `remove_file` / `create_directory`, feeding `changed_paths` / `take_changed_paths`
for the Review hook. The tracking seam exists; nothing snapshots content.

Checkpointing is also what makes unattended running safe, and therefore what makes the
approval-rules spec (item 10) defensible: "always allow `edit_file` in `src/`" is a reasonable
grant when every edit is revertable and a reckless one when it is not.

## Goals & Non-goals

**Goals**

- Snapshot the content of every file before the agent modifies it, per turn.
- `/undo` restores both the transcript **and** the files, atomically enough that the two never
  disagree.
- A `/revert` command to restore files without touching the transcript (the "that edit was
  wrong, try again with this context" case).
- Work in a git repo and outside one, without requiring a clean working tree and without
  touching the user's git state (no stashes, no commits, no index changes).
- Bounded disk usage with automatic cleanup.
- Never lose *user* edits made outside the agent.

**Non-goals**

- No time-travel beyond the session's checkpoints. Checkpoints are session-scoped and deleted
  on exit.
- No diff UI at v1 (`/revert --dry-run` lists paths; it does not render diffs — `diff_files`
  exists for that).
- No checkpointing of files outside the CWD jail. The jail is the boundary.
- No checkpointing of `exec_shell` side effects. A shell command that writes files is not
  tracked — stated as an explicit limitation, not silently.
- No conflict resolution UI.
- No desktop surface at v1.

## Design

### 1. Storage

Content-addressed blobs under the session directory:

```
~/.aictl/checkpoints/<session-id>/
  blobs/<sha256>                 # file contents, deduplicated
  turns/0003.json                # manifest for turn 3
```

Manifest:

```json
{
  "turn": 3,
  "created_at": "2026-09-10T10:14:02Z",
  "user_prompt_index": 12,
  "entries": [
    { "path": "src/run.rs",  "before": "sha256:ab12…", "mode": 33188 },
    { "path": "src/new.rs",  "before": null, "mode": null }
  ]
}
```

`before: null` means the file did not exist — restoring deletes it. `mode` preserves the
executable bit, which matters for scripts.

Content addressing means an unchanged file re-edited across five turns stores one blob. In a
typical coding session the whole checkpoint store is a few hundred KB.

**Deliberately not git.** `git stash create` was the report's suggestion and is tempting —
free content addressing, free diffing. It is rejected because: it requires a git repo (the
agent must work outside one), it writes objects into the user's repo that survive if cleanup
fails, it interacts badly with a dirty index or an in-progress rebase/merge, and it makes the
agent's undo semantics depend on the user's git state. A private blob store has none of those
failure modes and is ~150 lines.

### 2. Capture

Capture happens **before** a mutation, in the same place `record_workspace_change` is called —
but it must run before the write, not after, so it hooks at the tool-dispatch seam rather than
the post-dispatch one.

```rust
// crates/aictl-core/src/checkpoint.rs

/// Snapshot `path`'s current content into the active turn's checkpoint.
/// No-op when checkpointing is disabled, when the path is already captured
/// this turn, or when the file exceeds the size cap.
pub fn capture(path: &Path);
```

Called from `run::handle_tool_call` / `run_parallel_call` immediately before dispatching a
mutating tool, for each path the call will touch. Path extraction reuses the same logic the
security gate already applies to classify a tool's target — `security::validate_tool` must
already know which path a `write_file` / `edit_file` / `remove_file` body refers to, and that
extraction is factored out and shared rather than reimplemented.

Idempotent per turn: the first capture of a path in a turn wins, so a file edited three times
in one turn restores to its pre-turn state, not its pre-third-edit state. That matches what
`/undo` means (undo the *turn*).

Size cap `AICTL_CHECKPOINT_MAX_FILE_BYTES` (default 5MB): larger files are recorded in the
manifest as `skipped: true` and `/undo` reports honestly that they could not be restored
rather than silently leaving them modified.

### 3. Restore

```rust
/// Restore every file captured in turns > `keep_through` to its pre-turn
/// state. Returns the paths restored and the paths that could not be.
pub fn restore_to(keep_through: u32) -> RestoreReport;
```

Safety check before writing anything — the one case that must not go wrong is clobbering the
user's own edits:

For each entry, hash the file's **current** content. If it matches neither the `before` blob
nor the content the agent left behind (recorded as `after` in the manifest, hashed at
capture-time-plus-one), the file was modified outside the agent. Those paths are **not
restored**; they are listed in the report as conflicts, and the user is told:

```
  ⚠ 1 file changed outside the agent and was not restored:
      src/config.rs  (edit it manually or use /revert --force)
```

Restores are written to a temp file in the same directory and renamed, so a crash mid-restore
leaves either the old or the new content, never a truncated file.

### 4. `/undo` integration

`commands::undo` currently calls `transcript::undo_turns`. It becomes:

1. Compute which turns are being dropped.
2. `checkpoint::restore_to(target_turn)`.
3. Only if the restore reported no hard failures, `transcript::undo_turns`.
4. Print both: transcript turns dropped, files restored, conflicts skipped.

Ordering matters: restoring files first means a failed restore leaves the transcript intact
and consistent with disk. The reverse order would recreate exactly the inconsistency this
spec exists to fix.

`coding::clear_changed_paths()` and `coding::invalidate_repo_context()` are called after a
restore — the Review hook's changed-file list and the cached `<repo_context>` both refer to a
state that no longer exists.

### 5. `/revert`

New command for the case where the transcript should be kept:

```
/revert              # restore files from the last turn; keep the conversation
/revert --dry-run    # list what would be restored
/revert 2            # restore the last 2 turns' files
/revert --force      # restore even conflicted files (destructive; requires confirm)
```

After a bare `/revert` the agent still knows what it tried, and the user can say "that broke
X, do it differently" — which is the actual workflow. `/undo` is for "forget this happened".

### 6. Lifecycle

- A turn's checkpoint is created lazily on first capture — turns with no writes cost nothing.
- **Retention**: the last `AICTL_CHECKPOINT_KEEP_TURNS` (default 20) turns. Older manifests
  are deleted and their blobs garbage-collected (a blob is removed when no manifest references
  it).
- **Session end**: the whole `<session-id>` directory is deleted on clean exit, next to the
  existing `mcp::shutdown()` calls. On unclean exit it survives; a startup sweep removes
  directories for sessions older than `AICTL_CHECKPOINT_MAX_AGE_HOURS` (default 48), which
  also gives a crashed session a recovery window.
- **Incognito** (`session::is_incognito()`): checkpointing is **disabled**. Incognito means
  nothing about the conversation touches disk; writing file contents into a blob store would
  violate that directly. `/undo` in incognito reverts the transcript only and says so.
- **Redaction**: blobs are file contents the user already has on disk in the same form, so
  `redact_for_persistence` does **not** apply — redacting a checkpoint would make it unable to
  restore the file. The manifest's paths are the only new metadata, and they are already in
  the audit log. This is a deliberate, stated exception to the persistence-seam rule.

### 7. Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_CHECKPOINT_ENABLED` | `true` | Master switch (forced off in incognito) |
| `AICTL_CHECKPOINT_KEEP_TURNS` | `20` | Retained turns |
| `AICTL_CHECKPOINT_MAX_FILE_BYTES` | `5242880` | Per-file capture cap |
| `AICTL_CHECKPOINT_MAX_TOTAL_BYTES` | `104857600` | Store cap; oldest turns evicted first |
| `AICTL_CHECKPOINT_MAX_AGE_HOURS` | `48` | Startup sweep threshold for orphaned sessions |

### 8. CLI surface

- `/undo` output gains the file summary:
  ```
    ✓ undid 1 turn
    ✓ restored 3 files
      src/run.rs, src/tools.rs, docs/TOOLS.md
  ```
- `/revert` as specified in §5.
- `--info` gains `checkpoints: 4 turns, 312 KB`.
- Desktop: none at v1. (The desktop has its own undo affordance; wiring it is a follow-up
  that should land before the desktop enables broad auto-accept.)

### 9. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/checkpoint.rs` | **New** — blob store, manifests, capture, restore, GC, startup sweep |
| `crates/aictl-core/src/run.rs` | `capture` before mutating dispatch in both `handle_tool_call` and `run_parallel_call`; turn counter |
| `crates/aictl-core/src/security.rs` | Factor out the per-tool target-path extraction so capture and the gate share it |
| `crates/aictl-core/src/session.rs` | Session-end cleanup; incognito gate |
| `crates/aictl-core/src/coding.rs` | Clear changed-paths + bust repo context after a restore |
| `crates/aictl-core/src/config.rs` | Five keys |
| `crates/aictl-cli/src/commands/undo.rs` | Restore-then-trim ordering; new output |
| `crates/aictl-cli/src/commands/revert.rs` | **New** |
| `crates/aictl-cli/src/commands.rs`, `help.rs` | Register `/revert` |
| `crates/aictl-cli/src/main.rs` | Startup sweep; exit cleanup |
| `docs/USAGE.md`, `docs/CONFIG.md`, `docs/CODING_AGENT.md`, `CLAUDE.md` | Document, including the `exec_shell` limitation |

### 10. Testing

**Unit**

- Blob store: identical content stores one blob; GC removes unreferenced blobs and keeps
  referenced ones.
- Capture idempotence: three captures of one path in one turn keep the first content.
- Capture of a non-existent path records `before: null`; restore deletes the created file.
- Mode preservation: an executable script restores executable.
- Size cap: an over-cap file is marked skipped and reported, not silently dropped.
- Conflict detection: a file modified externally between capture and restore is reported as a
  conflict and left alone; `--force` restores it.
- Atomic restore: a simulated failure mid-restore leaves the original file intact (temp-file
  rename semantics).
- Retention: 25 turns with `KEEP_TURNS=20` leaves 20 manifests and GCs the orphaned blobs.
- Total-size cap evicts oldest first.
- Incognito: `capture` is a no-op and no directory is created.

**Integration**

- Mock-LLM: model writes two files across two turns; `/undo` restores turn 2's file to its
  prior content and leaves turn 1's alone; the transcript and disk agree.
- `/undo` when the restore hits a conflict: the transcript is **not** trimmed, and the user
  sees why. (This is the ordering guarantee from §4 — it deserves its own test.)
- `/revert` restores files and leaves `messages` untouched.
- A restore busts the repo-context cache: the next `<repo_context>` shows the restored dirty
  state.
- Session end deletes the checkpoint directory; a killed session leaves it and the next
  startup sweeps it after the age threshold.

**Manual smoke**

1. Multi-file edit, `/undo`, `git status` clean (in a repo that started clean).
2. Agent edits a file, user edits the same file in an editor, `/undo` → conflict reported,
   user's edit intact.
3. Large binary file edited by the agent → skipped with an honest message.

### 11. Rollout

1. **`checkpoint.rs` + blob store + capture wiring**, with `/undo` unchanged. Checkpoints
   accumulate and are tested directly; no user-visible behavior change.
2. **`/undo` restore integration** with the ordering guarantee and conflict reporting.
3. **`/revert`, `--info`, retention/GC/sweep, docs.**

### 12. Verification

1. `cargo build --workspace`, `cargo lint`, `cargo test` clean.
2. `AICTL_CHECKPOINT_ENABLED=false` reproduces today's `/undo` behavior exactly, with a
   warning line noting files are not restored (honest, since that is today's behavior).
3. Manual smoke checklist, especially the external-edit conflict case.
4. Disk: a 50-turn session with edits stays under a few MB; `~/.aictl/checkpoints/` is empty
   after a clean exit.
5. Incognito session creates nothing under `~/.aictl/checkpoints/`.
6. CI gate: `grep -rE 'checkpoint::' crates/aictl-server/src/` empty.

### 13. Risks

- **Clobbering user edits.** The one unacceptable failure. Mitigation: content-hash conflict
  detection (§3), conflicts never auto-restored, `--force` gated behind an explicit confirm,
  and the dedicated manual smoke test. This risk is why restore is hash-checked rather than
  blind.
- **Disk growth.** A session editing large generated files could balloon the store.
  Mitigation: per-file cap, total cap with oldest-first eviction, session-end deletion,
  startup sweep for orphans.
- **Partial coverage creating false confidence.** `exec_shell` running `sed -i` or a codegen
  script is not captured, so `/undo` will report success while leaving those changes.
  Mitigation: document prominently; `/undo` output says "restored N files tracked by the
  agent's file tools" rather than implying full coverage. A follow-up could snapshot the whole
  changed-file set via a pre/post `git status` diff when in a repo — noted, not specified.
- **Ordering bug reintroducing the inconsistency.** If a future refactor trims the transcript
  before restoring, the bug is back. Mitigation: the integration test in §10 asserts the
  transcript survives a failed restore.
- **Incognito leak.** Capturing in incognito would write user content to disk against an
  explicit user choice. Mitigation: the gate is checked in `capture` itself, plus a test.
- **Interaction with the git-aware `<repo_context>`.** A restore changes `git status` output
  the model has cached. Mitigation: explicit cache bust after restore.

### 14. Open questions

- **Should checkpoints survive the session by default?** A user who quits and reopens might
  want to undo yesterday's agent edits. Against: it is a surprising amount of retained user
  content, and the session-scoped model is easy to explain. Lean session-scoped with the
  48-hour crash-recovery window as specified; revisit if users ask.
- **Should `exec_shell` capture be attempted** via a pre/post `git status` diff when in a
  repo? It would close the biggest coverage gap. It also only works in a repo, only for
  tracked files, and adds a `git` call per shell command. Worth prototyping after v1.
- **Auto-checkpoint before a risky operation** (`remove_file` on a directory, a `git checkout`)
  even outside coding mode? Probably yes eventually; v1 keeps the capture seam on the four
  file tools only.
- **Desktop wiring** should land before the desktop offers broad auto-accept — flagged here so
  the dependency is not lost.
