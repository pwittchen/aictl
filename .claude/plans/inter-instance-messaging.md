# Plan: Inter-Instance Messaging for aictl

## Context

A user frequently has more than one `aictl` REPL open at once — one in each repo, one scratch instance for research, one running a long agent task, etc. Today these processes are fully isolated. The roadmap proposes letting them talk to each other: instance A sends a prompt, note, or context snippet to instance B; B receives it and can fold it into the next turn (or ignore it). This turns a pile of independent REPLs into a loose workstation-wide workflow — "hand the plan you just produced to the other window", "ping me when this test run finishes", "send the failing diff to the security-focused agent".

This plan covers the local, same-machine case only. Cross-machine messaging is explicitly out of scope — it would require auth, transport security, and presence semantics that single-machine IPC doesn't.

## Goals & Non-goals

**Goals**
- Let an `aictl` REPL discover the other currently-running `aictl` REPLs on the same machine and send them a text message.
- Deliver the message to the target instance with minimal latency and without corrupting its current prompt or in-flight agent turn.
- Give the receiving user a clear, non-surprising way to accept, decline, or defer each incoming message before it enters the conversation.
- Keep incoming messages under the same security gate (redaction, audit) as user input.
- Fail closed: if the transport is unavailable or a target is stale, the sender sees a clear error and nothing leaks.

**Non-goals**
- No cross-machine transport. Unix domain sockets only; TCP sockets, HTTP, gRPC are not in scope.
- No end-to-end encryption of on-disk socket paths — standard Unix permissions on `~/.aictl/ipc/` are the security boundary.
- No message history / offline delivery. If the target isn't running, the send fails; the sender decides whether to retry.
- No automatic conversation merging. Received messages never auto-inject into the target's message history without the user's consent (configurable — see §8).
- No pub/sub fan-out, group channels, or threads in v1. Point-to-point only. Broadcast (`/send all …`) can come later.
- No richer payloads (attachments, images, structured tool-call results) in v1. Text only.
- No Windows support. Named-pipe fallback is a follow-up phase, matching the rest of aictl's platform stance.

## Approach

### Transport

A Unix domain socket per running instance, under `~/.aictl/ipc/<session-id>.sock`. Each REPL:

1. On startup, binds a fresh `UnixListener` at `~/.aictl/ipc/<session-id>.sock`.
2. Spawns a `tokio::task` that `accept()`s connections and deserializes newline-delimited JSON frames into an internal `mpsc::Sender<InboundMessage>`.
3. Writes a sibling `<session-id>.json` metadata file next to the socket (see §2) so other instances can discover this one without needing to `connect()` to probe.
4. On shutdown (graceful or panic), removes both `<session-id>.sock` and `<session-id>.json`. A background "reaper" sweep (see §3) handles crashes that bypass the cleanup.

Per-instance listener rather than a shared hub because:
- **No daemon lifecycle to manage** — there's no "who started the hub" bootstrap race.
- **Natural authorization boundary** — the socket file's Unix perms scope access to the owning user, and killing the instance kills the endpoint.
- **Matches how the session-id already works** — each REPL already has a uuid under `~/.aictl/sessions/<uuid>`; the ipc endpoint reuses that identity.

### Addressing

A message is addressed to a target by either:

- **Session id** — the long uuid. Unambiguous, but nobody types these by hand.
- **Session name** — the human-readable name from the existing `session::name_for` / `session::id_for_name` system. This is what users actually type.

Both resolve through the existing `session.rs` name table. An unambiguous prefix match on the uuid is also accepted (`/send a3f4 …` expands to the full uuid as long as exactly one running instance matches), matching how `/session` already works.

## Design

### 1. Socket layout and wire protocol

Directory: `~/.aictl/ipc/` — mode `0700`, created lazily by any instance that needs it. The whole directory is best-effort: if the filesystem doesn't support Unix sockets (e.g., a network mount) the feature disables itself with a one-line warning, mirroring how optional features already fail open.

Per instance:
- `~/.aictl/ipc/<session-id>.sock` — Unix domain socket, mode `0600`.
- `~/.aictl/ipc/<session-id>.json` — metadata file, mode `0600`, containing:
  ```json
  {
    "session_id": "a3f4…",
    "session_name": "security-audit",
    "pid": 12345,
    "started_at": "2026-04-24T12:34:56Z",
    "model": "claude-sonnet-4-6",
    "agent": "security-auditor",
    "protocol_version": 1,
    "accepts": ["prompt", "note"]
  }
  ```
  `accepts` is the set of payload kinds the instance is willing to receive (see §5). Controlled by config, see §8.

Wire format (newline-delimited JSON, one frame per line):

```json
{
  "v": 1,
  "kind": "prompt" | "note" | "ping" | "pong",
  "from": { "session_id": "…", "session_name": "…", "pid": 1234 },
  "to":   { "session_id": "…" },
  "id":   "<uuid>",
  "body": "…",
  "meta": { "sent_at": "2026-04-24T12:34:56Z" }
}
```

No length-prefix framing — NDJSON is enough because we never embed newlines in `body` (sender escapes them). Keeps the parser trivial and debuggable with `socat`.

### 2. Discovery

`ipc::list_peers()` scans `~/.aictl/ipc/*.json`, parses each, filters out:
- Files whose `pid` no longer exists (cheap `kill(pid, 0)` check).
- Files older than `AICTL_IPC_STALE_SECS` (default 24h) — belt-and-braces for pid-reuse edge cases.
- The caller's own session id.

Reaping stale files: the caller of `list_peers()` best-effort-deletes the stale entries (both `.sock` and `.json`) as it encounters them — no separate daemon needed. One instance's startup also triggers a full sweep before binding its own listener.

### 3. Listener lifecycle

New module `src/ipc.rs`:

```rust
pub struct IpcEndpoint {
    socket_path: PathBuf,
    meta_path: PathBuf,
    _listener_task: tokio::task::JoinHandle<()>,
    inbox: tokio::sync::mpsc::Receiver<InboundMessage>,
}

pub async fn start(session_id: &str, session_name: Option<&str>) -> Result<IpcEndpoint, IpcError>;
pub async fn send(target: &Peer, frame: &OutboundFrame) -> Result<(), IpcError>;
pub fn list_peers() -> Vec<Peer>;

impl Drop for IpcEndpoint {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_file(&self.meta_path);
        // listener task is aborted via tokio when the handle drops
    }
}
```

`start()`:
1. `fs::create_dir_all` with mode `0700` on the ipc dir.
2. Reap obviously-stale `.sock`/`.json` pairs.
3. Bind the listener; refuse to bind if a `.sock` with our session id already exists (second instance with the same uuid is a bug, not a duplicate).
4. Write the metadata JSON.
5. Spawn the accept loop; each accepted connection is handed to a per-connection task that reads one NDJSON frame, validates it, pushes to the inbox channel, and closes.

One frame per connection intentionally — keeps the protocol stateless and avoids half-open leak paths. Throughput is irrelevant for this use case (human-driven).

Panic safety: the `Drop` cleanup runs on graceful shutdown, SIGTERM via a signal handler, and rustyline's ctrl-C path. A pure panic in the REPL loop also unwinds through `Drop`. A hard crash (SIGKILL, OOM) leaves files behind — the next instance's sweep reaps them.

### 4. REPL integration

The REPL's `run_interactive` loop currently does a blocking `rl.readline(&prompt)` call. Adding IPC without changing UX requires a way for inbox events to interrupt the prompt.

Two design options considered:

- **(A) Run `readline` on a dedicated thread, `select!` between the readline-reply channel and the inbox channel in the Tokio loop.** Inbox events can interrupt the prompt (redraw with an inline notification), wait for the current readline to finish, or be deferred entirely.
- **(B) Poll the inbox channel only at turn boundaries** — between `run_and_display_turn` calls. Simpler (no readline threading changes) but the user doesn't see incoming messages until they finish typing or hit Enter.

Start with **(B)**; it keeps the readline path untouched and is sufficient for the common case (a user who is thinking, not typing). If users ask for live interruption, migrate to **(A)** — the inbox channel already exists, only the readline bridge changes.

Concretely:

- At the top of each readline iteration, drain the inbox: pop up to N inbound messages, render them as banner lines above the prompt, and either queue them for user acceptance (default) or auto-accept into the next turn (configured per-instance, see §8).
- When the REPL produces a final answer for the turn, drain the inbox again before re-prompting so the user sees notifications soon after they land.
- Received messages do *not* write into `session.rs` until the user accepts them (or auto-accept is on).

### 5. Payload kinds

- **`prompt`** — treated as user input. On accept, injected as the next turn's user message (equivalent to the user typing it).
- **`note`** — informational; rendered as a banner but never injected into the model. Good for "ping, your test run finished".
- **`ping`** / **`pong`** — health-check frames used by `/send` to confirm the target is alive before offering delivery. Never user-visible.

Kind is advisory for the receiver — the sender declares intent, the receiver renders and routes accordingly. Future kinds (`skill`, `agent`, `file-ref`) can be added by bumping `protocol_version` and extending `accepts`.

### 6. Sender-side slash command

`/send <target> <text…>`:
- `<target>` resolves through: exact session name → unambiguous session-id prefix → error.
- `<text…>` everything after the target, raw (including spaces). Multi-line input uses the existing rustyline multi-line edit or `/send @<path>` to pull the body from a file (bounded by `AICTL_IPC_MAX_BYTES`, default 64 KB).
- Defaults to kind `prompt`. `/send --note <target> <text>` switches to the notification kind.
- Before writing to the socket, runs the body through the same outbound redactor used for provider dispatch. Sending a session full of secrets to another REPL shouldn't bypass the boundary control.
- Returns one line of feedback: `→ sent to 'security-audit' (pid 12345)` or a clear error (`no peer matched 'sec*'`, `peer 'security-audit' not responding`, `message rejected: body too large`).

`/peers` lists running peers (name, session-id short prefix, pid, model, agent, `accepts`). Useful before sending.

`--send <target>=<text>` CLI flag for scripted use (e.g., `aictl --send build-watcher="tests passing"` from a cron). `--list-peers` is the non-interactive discovery command.

### 7. Receiver-side presentation and accept UX

When a message lands and the REPL drains its inbox:

```
┌─ message from 'planner' (claude-sonnet-4-6, agent: planner) ────────────
│ Here is the plan I produced for the auth migration. Want to execute it?
│ 1. …
│ 2. …
└─ [a]ccept as prompt  [d]efer  [x]discard  [v]iew full  [q]uit menu
```

- **Accept** injects the text as the next user turn (appears as if the user typed it, styled so the origin is visible in history).
- **Defer** keeps it in the in-memory inbox; the banner replays next turn.
- **Discard** drops it and sends a `pong` receipt with `status: "discarded"` so the sender knows.
- **View full** renders the body in a pager if it was truncated in the banner.

In `--auto` mode the prompt is skipped only when `AICTL_IPC_AUTO_ACCEPT=true` is also set. `--auto` alone isn't enough; inbound messages are a privilege escalation equivalent to another user typing into your REPL, which `--auto` was never scoped to cover.

Notes (`kind: note`) skip the accept prompt — they only ever render as a banner, with `[d]ismiss [v]iew full`. They don't enter the conversation.

### 8. Security gate

Running against each inbound frame, in order:

1. **Transport identity** — Unix socket client is the same user (checked via `SO_PEERCRED`). Reject otherwise. On macOS, `LOCAL_PEERPID`/`LOCAL_PEEREUID` via `getsockopt`.
2. **Size cap** — `AICTL_IPC_MAX_BYTES` (default 64 KB) on the frame. Reject larger with `413`-like response.
3. **Rate limit** — per-sender-session token bucket, default 10 msgs/min. Excess frames get a `too_many_requests` reply and are dropped.
4. **Inbound redaction** — a received message is redacted before being surfaced (same `security::redaction` pipeline, direction `Inbound`). The sender's secrets don't leak into the receiver's conversation or logs.
5. **Accept gate** — the receiver's user has to approve before the message enters the conversation (except in the explicit `AICTL_IPC_AUTO_ACCEPT` case above).
6. **`security::validate_tool` does NOT apply** — these are messages, not tool calls. They go through the normal user-input path on accept, which means they can *produce* tool calls, which in turn hit the existing gate. No second validation tier needed.

Config key to disable entirely:
```
AICTL_IPC_ENABLED=false                 # master switch; default true on macOS/Linux
AICTL_IPC_ACCEPTS=prompt,note           # kinds this instance accepts; others rejected at frame parse
AICTL_IPC_AUTO_ACCEPT=false             # skip the y/N prompt on accepted kinds
AICTL_IPC_RATE=10/60                    # msgs per seconds window
AICTL_IPC_MAX_BYTES=65536               # per-frame cap
AICTL_IPC_STALE_SECS=86400              # reap metadata files older than this
AICTL_IPC_DIR=~/.aictl/ipc              # override for testing
```

Default master switch is **on** because the feature is local-only, user-scoped, and adds a clear value proposition. Unlike plugins and MCP, it doesn't execute third-party code. If the user's threat model includes other processes running as their user, they can set `AICTL_IPC_ENABLED=false`.

### 9. Audit

Every accepted, rejected, deferred, or discarded inbound message produces an audit entry under the existing `~/.aictl/audit/<session-id>/` tree, tagged `ipc_inbound`. Every `/send` produces an `ipc_outbound` entry on the sender side. Reuses the JSONL format and rotation that already exist for tool calls. Redacted body is what's logged — the raw body never hits disk.

### 10. Interaction with incognito mode

An instance launched with incognito (`session::is_incognito()`) still gets an ipc endpoint — incognito only governs persistence to `~/.aictl/sessions/`, not runtime IPC. However:
- The metadata file for an incognito instance sets `"name": null` and a throwaway session id so it's discoverable but not nameable.
- The receiver's audit log still records the message (audit is independent of session persistence — a different persistence boundary, per `session::is_incognito()` docs).

Config `AICTL_IPC_INCOGNITO=off` skips the endpoint entirely for paranoid users.

### 11. Interaction with `--quiet` and non-TTY

- `--quiet` suppresses inbound banners but still writes audit entries. Accept decisions fall back to `AICTL_IPC_AUTO_ACCEPT`; without that, unaccepted messages accumulate in the inbox and drop on session end.
- Non-TTY (piped stdout): same as `--quiet`. Single-shot invocations (`aictl --message …`) still bind an endpoint for the duration of the run so a sender can push context in — but the sender races the agent loop, so it's mostly useful for the scripted `--send` path on the other side.

### 12. Integration points

| File | Change |
|------|--------|
| `src/ipc.rs` | **New** — `IpcEndpoint`, `start`, `send`, `list_peers`, wire types, frame parser, peer credential check |
| `src/ipc/transport.rs` | **New** — Unix socket listener + client; isolated so a later Windows named-pipe impl slots in behind the same API |
| `src/main.rs` | `mod ipc`; `ipc::start(session::current_id(), session::current_info())` after session init; drop on shutdown; `--send` / `--list-peers` flags |
| `src/repl.rs` | Inbox drain at readline boundaries; banner rendering; accept prompt; route accepted prompts into the next turn |
| `src/commands.rs` + `src/commands/send.rs` + `src/commands/peers.rs` | New `/send` and `/peers` commands |
| `src/config.rs` | Readers for `AICTL_IPC_*` keys |
| `src/session.rs` | Minor: expose `session::current_info()` shape matching the metadata file (already close) |
| `src/security/redaction.rs` | Add `RedactionDirection::Inbound` + `RedactionSource::IpcMessage` if not already present |
| `src/audit.rs` | Add `Outcome::IpcInbound` / `Outcome::IpcOutbound` (or a parallel `log_ipc` helper) |
| `src/ui.rs` | Banner renderer for inbound messages; small accept-menu helper |
| `Cargo.toml` | No new deps needed — `tokio` already has `net::UnixListener` under the default features in use |

### 13. Testing

- **Unit tests** (`src/ipc.rs`):
  - Frame parse: happy, malformed JSON, missing fields, wrong protocol_version, oversize body.
  - Peer discovery: stale file reaping, pid-gone detection, self-exclusion.
  - Target resolution: exact name, unique uuid prefix, ambiguous prefix → error.
  - Rate limiter: token bucket refill, burst allowance.
- **Integration tests** (`tests/ipc.rs`):
  - Two in-process endpoints: A sends `prompt` to B, B's inbox receives the frame, accept-path injects as a turn input.
  - Crash simulation: endpoint leaks files, next `start()` reaps them.
  - Wrong-user connection rejected (hard to exercise on CI — keep as a unit test of the `SO_PEERCRED` wrapper with an injected `PeerCred`).
  - Large-body rejection.
  - Incognito endpoint discoverable but unnamed.
- **Manual smoke**:
  - Two REPLs open. `/peers` in each shows the other.
  - `/send other-session "hello"` in A; B shows the banner, accept injects the turn.
  - Kill A with SIGKILL, start a third REPL; `/peers` no longer lists A.
  - `aictl --send other-session="from a script"` from a shell while B is running.

### 14. Documentation

- New `docs/ipc.md`: transport, wire format, security model, example workflows (planner → executor, background watcher → active REPL, human broadcast).
- `ARCH.md` gains an "Inter-instance messaging" section with the dispatch diagram.
- `CLAUDE.md` one-paragraph pointer to `src/ipc.rs`.
- `ROADMAP.md`: remove the entry when v1 ships.
- `README.md` feature list + `/send`, `/peers` in the slash-command list.

## Rollout phases

1. **Phase 1 — transport + discovery.** `src/ipc.rs`, per-instance endpoint, metadata file, listener task, `list_peers`, stale reaping. No REPL integration yet; tested by spawning two instances and using `socat` to write a frame manually.
2. **Phase 2 — sender surface.** `/send`, `/peers`, `--send`, `--list-peers`. Messages land in the receiver's inbox channel but only print a debug banner.
3. **Phase 3 — receiver UX.** Banner rendering, accept/defer/discard menu, inject-as-turn path, `AICTL_IPC_AUTO_ACCEPT`.
4. **Phase 4 — security gate and audit.** Peer credential check, redaction, rate limit, audit entries, config keys.
5. **Phase 5 — docs, integration tests, mock smoke tests.** Ship.
6. **Phase 6 (optional, later) — live interruption** if users want the inbox to break into an active `readline`, move from turn-boundary drain to the dedicated-thread readline bridge (§4 option A).
7. **Phase 7 (optional, later) — richer payloads**: `kind: skill` to hand over a skill invocation; `kind: file-ref` to share a read-only path; broadcast (`/send all …`) with explicit confirmation.
8. **Phase 8 (optional, later) — Windows named-pipe** transport behind the same `ipc::transport` seam.

## Verification

1. `cargo build` and `cargo build --release` — clean.
2. `cargo lint` — no warnings.
3. `cargo test` — unit + integration tests pass.
4. Manual end-to-end: two instances, `/peers`, `/send`, accept path, discard path, banner on note kind, audit entries on both ends, incognito instance discoverable but unnamed, third-instance reap after kill -9.

## Open questions

- **Auto-accept granularity** — one flag for all kinds is too coarse. Before v1 ships, decide whether `AICTL_IPC_AUTO_ACCEPT` should be `notes` / `prompts` / `both`. Likely `notes` by default, `prompts` opt-in only.
- **Per-peer allowlist** — should a receiver be able to say "only accept from `planner` and `security-audit`"? Probably yes, eventually (`AICTL_IPC_ALLOW_PEERS=planner,security-audit`), but the session name is user-controlled and trivially spoofable when both ends run as the same user. Defer.
- **Name uniqueness** — two running instances could share the same session name (names aren't unique in `session.rs` today). `/send` either needs to reject ambiguous names or prefer the most recently touched one; I'd lean toward reject-with-list so the user picks. Revisit during Phase 2.
- **Readline interruption (option A vs. B)** — start with B, but collect feedback. If users frequently miss time-sensitive notes because they were mid-typing, Phase 6 is the fix.
- **Do we want `/reply`?** — when a banner is shown and the user accepts, should the receiver's next final answer automatically flow back to the sender as another `prompt`? Powerful but easy to misuse; defer until there's a concrete use case.
- **Interaction with the future server mode** — once `aictl-server` (per the roadmap) exists, should IPC connect REPL instances to an `aictl-server`? Probably no: server is network-addressed and the CLI already talks to it via HTTP. IPC is specifically for REPL↔REPL.
