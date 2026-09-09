# Spec: Diagnostics in the edit loop

> Report item **8** (Tier 3). Smallest change in the set with a direct effect on edit quality.
> Independent of every other spec.

## Context

`lint_file` exists ([`tools/lint.rs`](../../crates/aictl-core/src/tools/lint.rs), 576 lines)
with a per-extension registry covering Rust, Python, JS/TS, Go, Java, Kotlin, C/C++ and more.
`coding::detect_linter` / `detect_build_cmd` / `detect_test_cmd`
([`coding.rs`](../../crates/aictl-core/src/coding.rs)) resolve project-level commands.

But diagnostics only run in the **host Review hook** — `coding::run_structured_review`, which
fires when the model emits a no-tool-call response and the session has touched files. The
feedback loop is therefore:

```
edit → edit → edit → final answer → review runs → build fails →
<review_result> injected → model re-reads → fixes
```

The model learns about a typo in edit #1 three edits and one full LLM round-trip later, by
which point it has built two more edits on top of the broken state. Closing that loop to
zero turns — the error arrives *in the tool result of the edit that caused it* — is the
difference between "the agent fixed it before I saw it" and "the agent shipped a broken
patch and then noticed".

The Review hook stays; it is the whole-project gate. This spec adds a fast per-file check
that runs inline.

## Goals & Non-goals

**Goals**

- After a successful `write_file` / `edit_file` on a source file, run a **fast** syntax or
  type check scoped to that file and append the diagnostics to the tool result.
- Strict latency budget with a hard timeout — this runs on the critical path of every edit.
- Per-language check commands, reusing the `lint_file` registry rather than a second one.
- Clean output adds nothing (no "✓ no issues" noise on every edit).
- Off by default outside coding-agent mode; on by default inside it.
- Never fail an edit because the checker failed. Diagnostics are advisory; the write already
  happened.

**Non-goals**

- No LSP. Full language-server integration (go-to-def, find-refs, workspace diagnostics) is a
  much larger feature; this is the cheap slice that captures most of the value. See Open
  questions.
- No auto-fix. The model gets the errors and decides.
- No cross-file diagnostics. A change that breaks a *caller* is the Review hook's job — that
  is precisely the division of labor.
- No new tool. This is a host behavior attached to existing tools.
- No change to the Review hook.

## Design

### 1. The check registry

Extends the existing `lint_file` per-extension registry with a **second, faster tier**. The
distinction is latency: `lint_file` may run `clippy` (seconds to minutes); inline checks must
be sub-second.

```rust
// crates/aictl-core/src/coding.rs
/// A fast, file-scoped syntax/type check suitable for running inline after
/// every edit. Must be fast enough to sit on the edit critical path —
/// anything that compiles the whole project belongs in the Review hook.
pub struct InlineCheck {
    pub cmd: Vec<String>,       // {} placeholder replaced with the file path
    pub label: &'static str,
}

pub fn inline_check_for(path: &Path) -> Option<InlineCheck>;
```

Initial registry, probing PATH the same way `detect_linter` does:

| Extension | Command | Notes |
|-----------|---------|-------|
| `.rs` | `rustfmt --check --emit stderr {}` | Catches syntax errors; does **not** typecheck (see §2) |
| `.py` | `ruff check {}` → `python -m py_compile {}` | Ruff when present; compile check otherwise |
| `.ts` `.tsx` | `tsc --noEmit --pretty false {}` | Only when `tsconfig.json` exists; slow enough to need the timeout |
| `.js` `.jsx` | `node --check {}` | Syntax only |
| `.go` | `gofmt -e {}` | Syntax + format errors |
| `.json` | `python -c json.load` or a builtin parse | Cheap; do it in-process |
| `.yaml` `.yml` | in-process parse | Cheap |
| `.toml` | in-process parse | Cheap |
| `.sh` | `shellcheck -f gcc {}` | When present |
| `.java` | (none at v1) | `javac` on one file without a classpath is noise |
| `.c` `.cpp` | (none at v1) | Needs `compile_commands.json` plumbing; Review hook covers it |

Where the check is a pure parse (`json`, `yaml`, `toml`), do it in-process — no subprocess, no
PATH dependency, microseconds.

Empty registry entry means no inline check for that file type. That is a valid and common
answer; the Review hook still covers the file.

### 2. The Rust problem, stated honestly

Rust's cheap check is the weakest entry in the table. `rustfmt --check` catches syntax errors
but not type errors, and `cargo check` compiles the whole crate — far too slow for the edit
path on a workspace this size.

Options considered:

- `cargo check --message-format=json` filtered to the edited file: correct diagnostics, but
  the cost is a crate-level compile. On a cold `target/` this is minutes.
- `rustc --edition 2024 --emit=metadata -Zparse-only`: nightly-only.
- `rustfmt --check`: fast, syntax-only, always available with the toolchain.

**v1 ships `rustfmt --check`** and accepts that Rust type errors surface at Review rather than
inline. There is a real middle ground worth measuring: `cargo check` is fast *when warm*, and
the coding loop is exactly the warm case. A config key allows opting into it:

```
AICTL_CODING_INLINE_CHECK_RUST=rustfmt   # rustfmt | cargo-check | off
```

with `cargo-check` running under the same hard timeout, so a cold build simply times out and
degrades to no inline diagnostics rather than hanging the edit. If eval data shows
`cargo-check` wins on warm workspaces, the default flips.

This is stated in the spec rather than hidden because it is the one place where the feature's
value depends on a measurement not yet taken.

### 3. Wiring

In `run.rs`, in the post-dispatch path for `write_file` / `edit_file` — next to the existing
`coding::record_workspace_change` and `coding::invalidate_repo_context` calls, which already
fire exactly here:

```rust
if coding_agent_enabled() && config::inline_diagnostics_enabled()
    && matches!(call.name.as_str(), "write_file" | "edit_file")
    && result_indicates_success(&output.text)
{
    if let Some(diag) = coding::run_inline_check(&path).await {
        output.text.push_str(&format!("\n\n<diagnostics file=\"{path}\" check=\"{}\">\n{diag}\n</diagnostics>", check.label));
    }
}
```

Key properties:

- Runs **only on success**. A failed edit has nothing to check.
- Appends to the existing tool result; no new message, no extra turn.
- Silent on clean output — `run_inline_check` returns `None` when the checker exits 0.
- Bounded by `AICTL_CODING_INLINE_CHECK_TIMEOUT_MS` (default `3000`). Timeout → `None`, plus
  a one-shot `warn_global` so the user knows the check is being skipped rather than passing.
- Spawned with `security::working_dir()` and `scrubbed_env()`, like every other subprocess.
- Diagnostics output is truncated to a small budget (`2KB`) — a file with 200 errors should
  show the first few, not flood the result.

The parallel-batch path (`run_parallel_call`) does not need this: `write_file` / `edit_file`
are side-effect tools and never run in a parallel batch.

### 4. Prompt guidance

One paragraph in `SYSTEM_PROMPT_CODING`:

```
After an edit, the tool result may include a <diagnostics> block from a
fast syntax check of the file you just changed. Fix what it reports before
moving to the next edit — a diagnostic on the file you just wrote is
always caused by the edit you just made. Absence of a block means the fast
check passed or does not exist for that file type; it does not mean the
project builds.
```

The last sentence prevents the model from treating a clean inline check as a green build and
skipping the Review phase.

### 5. Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_CODING_INLINE_DIAGNOSTICS` | `true` (coding mode only) | Master switch |
| `AICTL_CODING_INLINE_CHECK_TIMEOUT_MS` | `3000` | Hard timeout per check |
| `AICTL_CODING_INLINE_CHECK_RUST` | `rustfmt` | `rustfmt` \| `cargo-check` \| `off` |
| `AICTL_CODING_INLINE_CHECK_CMD` | unset | Override: a single command with `{}`, applied to every edited file regardless of extension |

`AICTL_CODING_INLINE_CHECK_CMD` is the escape hatch for projects with a bespoke checker, and
it goes through the same shell validation as any other command.

### 6. CLI / desktop surface

- `--info` gains `inline-checks: on (3000ms)` under the coding block, alongside the existing
  `build:` / `lint:` / `test:` lines.
- `/coding status` lists the resolved inline check for the current project's dominant file
  type.
- No new command, no desktop UI. The diagnostics are model-facing.

### 7. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/coding.rs` | `InlineCheck`, `inline_check_for`, `run_inline_check`; in-process parsers for json/yaml/toml |
| `crates/aictl-core/src/run.rs` | Post-edit hook next to `record_workspace_change` |
| `crates/aictl-core/src/config.rs` | Four keys; prompt paragraph |
| `crates/aictl-cli/src/commands/{info,coding_agent}.rs` | Surface the resolved check |
| `docs/CODING_AGENT.md`, `docs/CONFIG.md`, `CLAUDE.md` | Document |

### 8. Testing

**Unit**

- `inline_check_for`: known extensions resolve; unknown extension → `None`; a checker missing
  from PATH → `None` (not an error); `AICTL_CODING_INLINE_CHECK_CMD` overrides everything.
- In-process parsers: valid JSON/YAML/TOML → `None`; invalid → a diagnostic naming the line.
- `run_inline_check`: exit 0 → `None`; exit non-zero → `Some(stderr)`; timeout → `None` plus
  one warning; output over 2KB truncated.
- Rust mode switch: `off` → `None`; `cargo-check` builds the right command.

**Integration**

- Mock-LLM fixture: model writes a Rust file with a syntax error; assert the `edit_file`
  result contains a `<diagnostics>` block, and that the model's next turn sees it in the same
  message (not a separate injected turn).
- Clean write appends nothing — the result is byte-identical to today.
- `AICTL_CODING_INLINE_DIAGNOSTICS=false` appends nothing.
- Non-coding mode appends nothing.
- A failed `edit_file` (no matching block) runs no check.

**Eval**

Tasks 4–5 (fix a compile error) and 6–8 (add a CLI flag) are the ones to watch. The metric is
turn count, not just pass rate: the hypothesis is that inline diagnostics reduce median turns
on edit-heavy tasks. If turn count does not drop, the feature is not earning its latency.

### 9. Rollout

Two PRs:

1. **Registry + `run_inline_check` + in-process parsers**, with a unit-test suite and no call
   site. Inert.
2. **Wiring + prompt paragraph + config + `--info`.** Flips it on for coding mode.

### 10. Verification

1. `cargo build --workspace`, `cargo lint`, `cargo test` clean.
2. Non-coding mode and `AICTL_CODING_INLINE_DIAGNOSTICS=false` both produce byte-identical
   tool results to today (regression gate).
3. Manual: introduce a deliberate syntax error via the agent in a `.rs`, `.py`, and `.ts`
   file; verify the diagnostic appears in the same tool result and the model fixes it without
   an intervening full-project build.
4. Latency: measure the added wall-clock per edit on this repo; it must be under ~300ms for
   `rustfmt` mode.
5. Eval turn-count comparison on tasks 4–8.
6. CI gate: `grep -rE 'inline_check' crates/aictl-server/src/` empty.

### 11. Risks

- **Latency on the critical path.** Every edit gets slower. Mitigation: the hard timeout, the
  "fast tier only" registry rule, and verification step 4 as an explicit budget. If a check
  cannot be reliably sub-second it does not belong in the registry.
- **False positives training the model to ignore diagnostics.** `rustfmt --check` reports
  *formatting* differences, not just syntax errors — a model that sees "line too long" after
  every edit learns to skip the block. **This is the most likely way the feature fails.**
  Mitigation: filter `rustfmt` output to parse errors only (it exits differently for a parse
  failure than for a formatting diff — verify and gate on that), and if the distinction is not
  cleanly available, prefer `cargo-check` or `off` for Rust rather than shipping noise.
- **Checker not installed.** `ruff`, `tsc`, `shellcheck` may be absent. Mitigation: PATH probe
  with a cached result per process, same as the existing `rg` probe; absence is silent.
- **Diagnostics on a file the model intentionally left broken** mid-refactor (edit 1 of 3).
  Mitigation: the block is advisory; the prompt says fix before moving on, but nothing blocks.
  Accept some noise here — a mid-refactor broken state is genuinely worth flagging.
- **Security.** Running a checker means spawning a subprocess derived from a file path the
  model chose. Mitigation: the path already passed `security::check_path_with` during the
  write; the command comes from the host's registry, not the model; `AICTL_CODING_INLINE_
  CHECK_CMD` goes through shell validation.

### 12. Open questions

- **The Rust check** (§2) is the open decision. Measure `cargo check` warm-path latency on
  this workspace before locking the default.
- **Should diagnostics also run after `remove_file`?** Deleting a file breaks its importers —
  which is a cross-file concern, so no; the Review hook covers it.
- **Full LSP.** The right long-term answer for go-to-def, find-refs, and workspace-aware
  diagnostics, and a genuinely large feature (server lifecycle, capability negotiation,
  per-language config, document sync). Worth a spec of its own once this cheap slice has
  demonstrated that the model *acts* on diagnostics it is given. If it does not act on the
  cheap ones, it will not act on the expensive ones either — which makes this spec a useful
  probe for whether LSP is worth building.
- **Should the diagnostic block count against the Review hook's retry budget?** No — they are
  independent loops. But if a file has an unresolved inline diagnostic when the model tries to
  finish, the Review hook will catch it anyway, which is the correct redundancy.
