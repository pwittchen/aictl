# Spec: Background processes

> Report item **6** (Tier 2). Independent of the other specs; the largest new *security*
> surface in the set.

## Context

`exec_shell` blocks. It spawns a command, waits under `security::shell_timeout` (default 30s),
and returns the output. `tools/shell.rs` is 40-odd lines around `tokio::process::Command`
with `truncate_output` on the result.

That makes a whole class of work impossible:

- Start a dev server and then exercise it (`npm run dev` + `fetch_url localhost:3000`).
- Kick off a slow test suite and keep reading code while it runs.
- Tail a log while reproducing a bug.
- Run a watch-mode build.

Today the agent either times out, or the user runs the process themselves in another terminal
and the agent is blind to it. The `check_port` tool exists precisely because the agent so
often needs to know whether *someone else's* server is up.

Long-lived child processes are also where a tool sandbox is easiest to get wrong: a process
that outlives the session, a `kill` that reaches the wrong pid, a log buffer that grows
without bound.

## Goals & Non-goals

**Goals**

- Start a command in the background, poll its accumulated output, and stop it — three tools
  over one host-side process registry.
- Bounded output buffers per process with the same head/tail elision semantics as the
  context-management spec.
- Hard limits on concurrent processes and total lifetime.
- Every background process killed on turn end, session end, and process exit — including
  panics and `SIGINT`.
- Full security-gate coverage: a background command passes exactly the same
  `security::validate_tool` shell validation as `exec_shell`, plus its own confirmation.
- Visibility: the user can always see what is running and kill it.

**Non-goals**

- No interactive stdin at v1. Processes get `/dev/null` on stdin. (An agent driving a REPL is
  a different, much larger feature.)
- No process persistence across sessions. Nothing survives `aictl` exiting.
- No pty allocation. Programs that require a tty get their non-tty behavior; that is a
  documented limitation, not a bug to work around.
- No process groups / job control beyond kill-the-tree.
- No desktop UI at v1 beyond the existing tool-status surface.
- No server changes.

## Design

### 1. Registry

```rust
// crates/aictl-core/src/tools/background.rs
pub(crate) struct BgProcess {
    pub id: u32,
    pub command: String,
    pub child: tokio::process::Child,
    pub started: std::time::Instant,
    /// Ring-buffered combined stdout+stderr, capped at BG_BUFFER_BYTES.
    pub output: Arc<Mutex<RingBuffer>>,
    /// Byte offset the model has already been shown, so `poll` returns
    /// only what is new.
    pub read_cursor: usize,
    pub exit: Option<std::process::ExitStatus>,
}
```

Held in a process-global `Mutex<HashMap<u32, BgProcess>>`. Ids are small monotonic integers
(`1`, `2`, …) — the model has to type them, and pids are both unwieldy and dangerous to
expose (a model that learns pids may try `kill` via `exec_shell`).

Each process spawns two reader tasks (stdout, stderr) that append into the shared
`RingBuffer`. The ring buffer is the key design choice: a dev server left running for ten
minutes must not accumulate 200MB. It keeps the **last** `BG_BUFFER_BYTES` (default 256KB)
and counts what it dropped, so `poll` can say `(4.2MB of earlier output dropped)`.

### 2. Tools

Three tools, `TOOL_COUNT` +3.

```
<tool name="bg_start">
npm run dev
</tool>
→ started background process 1: `npm run dev`
  poll it with `bg_output 1`; stop it with `bg_stop 1`

<tool name="bg_output">
1
</tool>
→ <bg_output id="1" status="running" elapsed="12s" new_bytes="1840">
  … only output since the last poll …
  </bg_output>

<tool name="bg_output">
1 --wait 30 --until "Server listening"
</tool>
→ blocks up to 30s, returning as soon as the pattern appears in new output
  (or on exit, or on timeout)

<tool name="bg_stop">
1
</tool>
→ stopped process 1 (`npm run dev`), exit: killed after 4m12s
```

`--wait` / `--until` on `bg_output` is what makes the feature usable rather than a
poll-in-a-loop token sink. Without it a model starting a dev server burns four turns polling
until the port opens. `--until` is a plain substring match, not a regex — regex on model-
supplied input against a live stream is a footgun for no benefit here.

`bg_stop` with no argument stops all processes. `bg_output` with no argument lists all
processes with status and elapsed time.

Exit handling: when a process exits, its entry stays in the registry (status `exited(N)`)
until read once, then is reaped. A process that exits immediately with an error must not
vanish before the model can see why.

### 3. Security

Background processes reuse the shell gate wholesale. `security::validate_tool` gains arms
for the three tools:

- `bg_start`: identical validation to `exec_shell` — allowed/blocked command lists, subshell
  blocking, CWD jail for any path arguments. Same code path, not a parallel implementation.
- `bg_output` / `bg_stop`: id must be a registry key. Nothing else is accepted; there is no
  path to killing an arbitrary pid.

Spawn conditions match `exec_shell` exactly: `security::working_dir()` as cwd,
`security::scrubbed_env()`, no shell interpolation beyond what `exec_shell` already allows.

Additional limits:

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_BG_ENABLED` | `false` | Master switch. **Off by default** — long-lived child processes are opt-in, matching the MCP/plugins precedent |
| `AICTL_BG_MAX_PROCESSES` | `3` | Concurrent process cap |
| `AICTL_BG_MAX_LIFETIME_SECS` | `1800` | Hard kill after this, whatever the state |
| `AICTL_BG_BUFFER_BYTES` | `262144` | Per-process ring buffer size |
| `AICTL_BG_MAX_WAIT_SECS` | `120` | Ceiling on `--wait` |

Defaulting `AICTL_BG_ENABLED=false` is deliberate and consistent with how this codebase
treats every capability that spawns or connects to something the user did not directly
authorize (`AICTL_MCP_ENABLED`, `AICTL_PLUGINS_ENABLED` are both default-off). Coding-agent
mode does **not** flip it on implicitly.

### 4. Cleanup — the part that must not be wrong

Three independent mechanisms, because any one of them can fail:

1. **`kill_on_drop(true)`** on every spawned `Child`, matching the MCP stdio client's
   approach. Covers panic, `?`-propagation, and normal drop.
2. **Explicit `background::shutdown_all()`** called from every exit path that already calls
   `mcp::shutdown()` — `main`'s normal exit, the `SIGINT` handler, and the REPL's `/exit`.
   `grep -n "mcp::shutdown" crates/aictl-cli/src/` enumerates them.
3. **Turn-end sweep.** At the end of `run_agent_turn`, any process older than
   `AICTL_BG_MAX_LIFETIME_SECS` is killed with a note appended to the turn result. A watchdog
   task ticks every 10s to enforce the lifetime cap even inside a long turn.

Kill semantics: `SIGTERM`, wait 2s, `SIGKILL`. On Unix, kill the process *group* (spawned with
`setsid` so `npm run dev`'s child node process dies too) — the single most common real-world
leak is a shell wrapper exiting while its child keeps the port. On Windows the desktop app is
macOS-only today and the CLI's Windows support is untested here; use `Child::kill()` and note
the limitation.

**Session-end verification is a test, not a hope**: an integration test spawns a `sleep 300`,
drops the runtime, and asserts the pid is gone.

### 5. Interaction with the agent loop

`bg_start` and `bg_stop` are side-effect tools (never parallelized, always confirmed).
`bg_output` is **read-only and parallelizable** — polling two processes in one turn is
exactly the kind of batch the Phase 4 work enables — but only when `--wait` is absent. A
waiting poll blocks and must not be batched; `tools::is_parallelizable` inspects the body for
`--wait` the same way it already inspects `git` for `commit`.

Output from `bg_output` goes through the context-management budget like any other tool
result. The ring buffer is the *first* bound; the result budget is the second.

### 6. Prompt guidance

Added to both `SYSTEM_PROMPT` and `SYSTEM_PROMPT_CODING` (the capability is universal), only
when `AICTL_BG_ENABLED` is on — the catalogue is built dynamically, so a disabled feature
costs zero prompt tokens:

```
BACKGROUND PROCESSES. Use `bg_start` for anything that does not terminate
on its own — dev servers, watchers, log tails. Use `exec_shell` for
everything that finishes. After starting a server, wait for it to be ready
with `bg_output <id> --wait 30 --until "<a string the server prints>"`
rather than polling repeatedly. Always `bg_stop` a process when you are
done with it; do not leave servers running for the user to find.
```

### 7. CLI surface

- `/bg` slash command: lists running processes with id, command, elapsed, status; `/bg stop
  <id>` and `/bg stop all` kill them. This is the user's escape hatch and is required — a
  user must never have to hunt for a pid the agent started.
- `--info` gains `background: enabled (2/3 running)` or `background: disabled`.
- The REPL banner warns on exit if processes were force-killed:
  `⚠ stopped 2 background processes on exit`.

### 8. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/tools/background.rs` | **New** — registry, `RingBuffer`, spawn/poll/stop, watchdog, `shutdown_all` |
| `crates/aictl-core/src/tools.rs` | Register three tools; `TOOL_COUNT` +3; `SIDE_EFFECT_TOOLS` for start/stop; `is_parallelizable` body inspection for `--wait` |
| `crates/aictl-core/src/security.rs` | `validate_tool` arms reusing the `exec_shell` shell validation |
| `crates/aictl-core/src/run.rs` | Turn-end sweep |
| `crates/aictl-core/src/config.rs` | Five keys; dynamic catalogue entries; prompt guidance |
| `crates/aictl-cli/src/main.rs` | `shutdown_all()` on every exit path next to `mcp::shutdown()` |
| `crates/aictl-cli/src/commands/bg.rs` | **New** — `/bg` |
| `crates/aictl-cli/src/repl.rs` | Exit warning |
| `docs/TOOLS.md`, `docs/CONFIG.md`, `docs/CODING_AGENT.md`, `CLAUDE.md` | Document, including the default-off rationale |

### 9. Testing

**Unit**

- `RingBuffer`: under capacity retains everything; over capacity retains the tail and reports
  the dropped byte count; UTF-8 boundaries respected on the drop edge.
- Read cursor: two successive polls return disjoint content; a poll after no new output
  returns empty with `status="running"`.
- Registry caps: starting a 4th process with the cap at 3 errors with a message naming the
  running ids.
- `--wait` parsing: clamped to `AICTL_BG_MAX_WAIT_SECS`; `--until` substring matched against
  new output only.
- `is_parallelizable`: `bg_output 1` → true; `bg_output 1 --wait 5` → false.
- Security: a `bg_start` body that `exec_shell` would reject is rejected identically (table
  test over the existing shell-validation cases).

**Integration**

- Spawn `sleep 300`, assert it is running, `bg_stop`, assert the pid is gone within 3s.
- Spawn a process that prints a line then sleeps; `bg_output --wait 10 --until "ready"`
  returns in well under 10s.
- Spawn a process that exits immediately with status 1; the model's next `bg_output` shows
  `status="exited(1)"` and the stderr — i.e. it was not reaped before being read.
- Process-group kill: spawn `sh -c 'sleep 300 & wait'`, stop it, assert the inner `sleep` is
  also gone. This is the leak test that matters.
- Lifetime cap: set `AICTL_BG_MAX_LIFETIME_SECS=2`, spawn `sleep 60`, assert it is killed by
  the watchdog and the turn result carries the note.
- Runtime drop: spawn, drop the runtime without calling `shutdown_all`, assert the pid is gone
  (the `kill_on_drop` backstop).

**Manual smoke**

1. `npm run dev` in a scratch project, `--wait --until` for the ready line, `fetch_url
   localhost:3000`, `bg_stop`. Verify the port is released.
2. Ctrl-C the CLI mid-turn with two processes running; verify both die.
3. `/bg stop all` from the REPL.

### 10. Rollout

1. **Registry + `RingBuffer` + cleanup machinery**, no tools registered. All of §9's
   process-lifecycle tests pass here — this is the risky half and it lands testable and
   inert.
2. **The three tools + security arms + prompt guidance**, behind `AICTL_BG_ENABLED`.
3. **`/bg`, `--info`, exit warning, docs.**

### 11. Verification

1. `cargo build --workspace`, `cargo lint`, `cargo test` clean.
2. `AICTL_BG_ENABLED=false` (the default) leaves the tool catalogue and system prompt
   byte-identical to today.
3. All process-lifecycle tests green, including the process-group leak test.
4. Manual smoke checklist.
5. `ps` shows no orphans after a session that started three processes and crashed
   (`kill -9` the CLI mid-turn — the one case `kill_on_drop` cannot cover; document that the
   OS reparents and the processes do leak, which is why the lifetime cap exists).
6. CI gate: `grep -rE 'background::|bg_start' crates/aictl-server/src/` empty.

### 12. Risks

- **Orphaned processes.** The headline risk. A leaked dev server holding port 3000 is a real
  user-visible harm. Mitigation: three independent cleanup mechanisms (§4), the process-group
  kill, the lifetime cap, the `/bg` escape hatch, and the exit warning. Documented residual
  risk: `SIGKILL` on the CLI itself leaks until the lifetime cap would have fired — nothing
  can cover that, and the docs say so.
- **Security surface.** A long-lived process outlives the turn's approval. The user approved
  "start this", not "keep running for 30 minutes". Mitigation: default-off; the lifetime cap;
  `/bg` visibility; the exit warning. Worth stating in docs that a background process is a
  *standing* grant, unlike `exec_shell`'s momentary one.
- **Models polling in a loop.** Without `--wait`, a model burns turns polling. Mitigation:
  `--wait` exists and the prompt tells the model to use it; the duplicate-call guard already
  blocks back-to-back identical `bg_output` calls, which is exactly the pathological pattern.
- **Buffer loss confusing the model.** A model that misses the error because it scrolled out
  of the ring buffer will misdiagnose. Mitigation: the dropped-byte count is stated in every
  poll result; 256KB is generous for the intended uses.
- **Port conflicts across sessions.** Two `aictl` sessions each starting a dev server on 3000.
  Not the agent's problem to solve, but the failure message from the second one should be
  legible — `check_port` already exists and the prompt can mention it.
- **`--until` never matching.** Blocks for the full `--wait`, wasting wall-clock. Bounded by
  `AICTL_BG_MAX_WAIT_SECS`; the result says whether it matched or timed out.

### 13. Open questions

- **Should coding-agent mode default `AICTL_BG_ENABLED` to true?** It is the mode where the
  feature is most useful, and the mode is already opt-in and experimental. Lean no for v1 —
  the process-leak risk is qualitatively different from a prompt change, and two nested
  defaults are hard to reason about. Revisit after the leak tests have soaked.
- **stdin.** Some servers need a keypress to reload. Deferred; if it lands, it is a fourth
  tool (`bg_input`) and not a flag on `bg_start`.
- **Should `bg_output` auto-inject on turn start?** Appending "process 1 printed 40 new lines"
  to the prompt each turn would help the model notice a crash, but it is unsolicited context
  growth. Lean no; a one-line status (not content) in the `<repo_context>`-adjacent block is
  a cheaper middle ground worth prototyping.
- **Windows.** The process-group kill is Unix-specific. The CLI nominally builds on Windows;
  the honest v1 answer is `Child::kill()` plus a documented limitation, not a half-tested
  job-object implementation.
