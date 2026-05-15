# Plan: Coding Agent — Phase 2 (better edit and search)

## Context

Phase 1 ([`coding-agent.md`](coding-agent.md)) shipped the master switch, `SYSTEM_PROMPT_CODING`, the `WorkflowPhase` state machine, and the CLI + desktop UI surfaces. The mode now *steers* the agent through Explore → Plan → Code → Review → Test, but it still leans on the same general-purpose tool catalogue every other session uses:

- `edit_file` ([`crates/aictl-core/src/tools/filesystem.rs:128`](../../crates/aictl-core/src/tools/filesystem.rs)) is a strict find-and-replace: one match exactly, no line addressing, no multi-edit, no fuzzy fallback. Real edits often require either rewriting the whole file or stringing together three `edit_file` calls. Whitespace drift between what the model "remembers" and what's on disk silently breaks the match.
- `search_files` ([`filesystem.rs:77`](../../crates/aictl-core/src/tools/filesystem.rs)) walks `glob("dir/**/*")` and grep's each file with a single `contains` call. No regex, no `.gitignore` respect, no binary detection beyond "did `read_to_string` succeed", no case folding, no line context, no max-matches cap beyond the global output truncation. On medium-sized repos it's slow and noisy.
- `find_files` ([`filesystem.rs:170`](../../crates/aictl-core/src/tools/filesystem.rs)) is a glob, but a single glob — no `--type`, no exclusions, no `.gitignore`.
- `read_file` returns the raw body without line numbers, so the model has to count lines manually before calling `edit_file` and gets it wrong often. There's also no way to ask for "lines 200–260" — the whole file is read or nothing is.

Phase 2 closes those gaps. The work is bounded: smarter `edit_file`, ripgrep-backed search/find, and selective reading. Nothing here changes the agent loop, the security gate, the session format, or the system prompt seam — Phase 2 is purely a tool-surface refresh that the coding-mode prompt then directs the model toward.

## Goals & Non-goals

**Goals**

- Make `edit_file` reliable enough that the model rarely needs to fall back to `write_file` for a localized change.
- Replace the glob-and-`contains` search path with ripgrep when available; fall back to today's pure-Rust implementation when it isn't (no new hard dependency).
- Let the model ask for a slice of a file ("read lines 120–180") and always render line numbers so `edit_file` line-addressed edits land where the model expects.
- Update `SYSTEM_PROMPT_CODING` so the model knows the new tool shapes; update `BUILTIN_TOOLS` descriptions; update the CLI `/tools` printer and desktop Settings panel (both consume `BUILTIN_TOOLS` directly).
- Stay tool-compatible: existing inputs (single old/new block, glob-only `find_files`, raw `search_files` pattern) keep working. New affordances are additive.

**Non-goals**

- No new tools land — every change extends an existing tool. The `TOOL_COUNT` constant stays at 35.
- No editor-script "patch" tool (the LSP-style multi-file diff path is Phase 3+ territory if we ever want it).
- No language-server integration. Diagnostics still come from `lint_file`.
- No code-aware AST search (tree-sitter, semgrep). Ripgrep on text + the `.gitignore`-aware path covers the realistic-cost wins; AST search is a separate effort.
- No "auto-apply patch" or "suggested edits" UI. The agent still emits one `edit_file` call per change.
- No background indexing. Searches are stateless, just like today.
- Server stays untouched — same as Phase 1.

## Design

### 1. Smarter `edit_file`

Today the body grammar is exactly:

```
<path>
<<<
<old>
===
<new>
>>>
```

with a single occurrence count check. Phase 2 extends the grammar three ways while keeping the existing shape valid:

1. **Multi-edit**: the body may contain *more than one* `<<< … === … >>>` block. They apply in order, top-to-bottom. Each block keeps the same uniqueness rule (old text must match exactly once at the time of application). If any block fails, the whole edit aborts and no write happens — we don't want half-applied edits leaving the file inconsistent.
2. **Line-number addressing**: an optional `@<start>` or `@<start>-<end>` directive on the line right after the path scopes the search to that line range. Example:
   ```
   src/lib.rs
   @42-58
   <<<
   fn old() {
   ===
   fn new() {
   >>>
   ```
   This makes the model's mental model of "I want to change *that* function, the one at line 47" matchable on disk even when the same name appears elsewhere. Line numbers are 1-based and inclusive; out-of-range fails fast.
3. **Fuzzy match fallback**: when an exact match fails *and* the whitespace-only diff hits zero, the tool retries with whitespace normalized (runs of spaces/tabs collapse to a single space, leading/trailing whitespace per line trimmed). On a fuzzy hit the tool re-anchors using the original surrounding bytes and writes the file using the model's `new` text verbatim — i.e. fuzzy is used *only to locate*, never to rewrite. If the fuzzy match isn't unique either, the tool surfaces a "found N near matches, candidates: <first line previews>" error rather than guessing.

Implementation:

- Parser refactor: split `tool_edit_file` into `parse_edit_body` (returns `Vec<EditBlock>` with optional line range) and `apply_edit_blocks` (reads, applies in sequence, writes once). Both are pure functions modulo `tokio::fs`.
- Keep the existing error strings ("old text not found in file", "old text found N times") for the single-block case so any tests / model habits stay compatible. The new errors ("line range out of bounds", "fuzzy match found N candidates") are additive.
- Truncate `EditBlock.old` / `EditBlock.new` previews to ~80 chars in error messages so a 2 KB block doesn't dump into the assistant message.

Files touched:

- `crates/aictl-core/src/tools/filesystem.rs` — `tool_edit_file` + new helpers + tests.
- `crates/aictl-core/src/tools.rs` — update the `edit_file` description in `BUILTIN_TOOLS` to mention multi-edit + line addressing.
- `crates/aictl-core/src/config.rs` — update the `edit_file` section of `SYSTEM_PROMPT` *and* `SYSTEM_PROMPT_CODING` so the model knows the new grammar.

Tests:

- Single block today's syntax still works (regression).
- Two-block edit applies both in order; verifies file on disk has both changes.
- Two-block edit where the second block fails reverts to the original file (no partial write).
- `@start-end` scopes the match: a `fn foo()` at line 10 stays untouched, the `fn foo()` at line 50 gets edited.
- Fuzzy fallback: leading whitespace differs by one space; tool still finds the unique match and rewrites.
- Fuzzy fallback non-unique: returns the "N candidates" error.

### 2. Ripgrep-backed `search_files`

Probe for `rg` on `PATH` at first use; cache the result in a `OnceLock<bool>`. When present, run `rg --no-heading --line-number --color=never --hidden --ignore-case=smart` (configurable via input flags below) with the user's pattern and the optional directory. When absent, fall back to today's `glob` + `contains` implementation so no install is required for the tool to work.

Input grammar (additive, all flags optional, today's `<pattern>\n<dir>` keeps working):

```
<pattern>
[--regex | --literal]      (default: --literal — preserves today's contains semantics)
[--case sensitive|smart|insensitive]    (default: smart)
[--type rust|py|js|…]      (passes -t<type> to rg)
[--max <N>]                (max matches across the run; default 200; cap at 1000)
[--context <N>]            (lines of context around each match; default 0)
[<dir>]                    (defaults to "." — same as today)
```

Behavior:

- Output shape stays line-prefixed `<path>:<line>:<text>` so model habits keep working. Context lines (when `--context` > 0) use rg's `--` separator between hunks, which we pass through.
- Respect `.gitignore` by default (rg's default). A `--no-ignore` flag is available for the rare case the model needs to grep generated files.
- Pin the working directory to `security::working_dir()` (already enforced by `validate_tool` → `check_path`; we just need to spawn the subprocess with that cwd).
- Cap output at `MAX_TOOL_OUTPUT_LEN` exactly like today; `--max` is a *secondary* cap so an aggressive search doesn't waste rg time before the cap kicks in.

The fallback path (no `rg`) only honors `--regex` to the extent of compiling a `regex::Regex` (already a transitive dep through `redaction` if `redaction-ner` is off — confirm in Cargo.lock; otherwise add `regex` as a direct dep on `aictl-core`). Other flags are documented as "best with ripgrep installed" and silently no-op in fallback.

Files touched:

- `crates/aictl-core/src/tools/filesystem.rs` — new `tool_search_files_rg` + the existing `search_files_blocking` kept as fallback + a `cfg_rg` probe.
- `crates/aictl-core/src/tools.rs` — update description, no dispatch change.
- `crates/aictl-core/src/config.rs` — `SYSTEM_PROMPT_CODING` Explore-phase guidance: "prefer search_files with --regex / --type when looking for symbols".

Tests:

- Pattern + dir today's shape returns matches in expected format.
- `--regex` switches semantics; an invalid regex returns "Error: invalid regex: …" (rg surfaces this with a non-zero exit).
- `--type rust` filters to `.rs` files only (skipped under "no rg" with a clear note).
- Fallback path engages when `rg` is not on PATH (test-only override via env var `AICTL_TEST_FORCE_RG_FALLBACK=1`).

### 3. Ripgrep-backed `find_files`

Same probe. When `rg` is available, run `rg --files <dir>` + filter by glob via `globset` (already a transitive dep through `glob` or add directly). When absent, today's `glob::glob` path stays.

Why use rg for file listing? Two reasons: it respects `.gitignore` (so `find_files **/*.rs` doesn't dump `target/` matches), and it's much faster on large trees. The glob filter sits on top of rg's output so the model's existing `**/*.rs` patterns keep working unchanged.

New optional input flag (additive):

```
<pattern>
[--type rust|py|…]
[<dir>]
```

When `--type` is provided, skip the glob altogether and pass `-t<type>` to rg directly; otherwise glob-filter the file list.

Files touched: same as `search_files`. Tests mirror the pattern.

### 4. Selective `read_file` and line numbers

Two changes, both additive:

1. **Line-number rendering**: when the input contains a `--lines` flag, the body is prefixed with the line number for each line, in `<5-digit-padded line>: <content>` format. Without the flag, output is unchanged.

   Rationale: prefixing every read by default would break callers that pass the body straight to `edit_file` (since `edit_file`'s `old` block would now start with `00042: `). Opt-in by flag means the model asks for numbered output when it intends to use line addressing, and the existing flow keeps working.

2. **Range read**: `--lines <start>[-<end>]` reads only the requested span. End is inclusive; out-of-range clamps (e.g. `--lines 500-9000` on a 600-line file returns lines 500–600 with a trailing `(end of file at line 600)` note).

Input grammar (existing path-only input remains the default):

```
src/lib.rs                       — full file, no line numbers (today)
src/lib.rs\n--lines              — full file, with line numbers
src/lib.rs\n--lines 120-180      — slice with line numbers
src/lib.rs\n--lines 120          — single line with line number
```

Files touched:

- `crates/aictl-core/src/tools/filesystem.rs` — `tool_read_file`.
- `BUILTIN_TOOLS` description update.
- Coding-mode prompt: "use --lines when you plan to edit by line range".

Tests:

- Plain `src/lib.rs` returns full file unchanged.
- `src/lib.rs\n--lines` returns full file with `NNNNN: ` prefix.
- `src/lib.rs\n--lines 5-10` returns lines 5–10 inclusive.
- `src/lib.rs\n--lines 9999-10000` returns "(file ends at line N, no content)".
- `src/lib.rs\n--lines 0` returns a clear "lines are 1-based" error.

### 5. Coding-mode prompt updates

`SYSTEM_PROMPT_CODING` ([`crates/aictl-core/src/config.rs`](../../crates/aictl-core/src/config.rs)) gets three small revisions:

- The `edit_file` section is rewritten to document the multi-block grammar and the `@start-end` directive, with a short example showing two blocks in one call. Adds: "If the file changed since you read it, re-read the relevant range with `--lines` and try again."
- The Explore-phase guidance learns the `search_files --regex / --type` flags and `find_files --type` flag.
- The Code-phase guidance learns the read-with-`--lines` pattern: "before editing a function you haven't read yet this turn, `read_file path\n--lines <range>` to confirm the exact bytes."

`SYSTEM_PROMPT` (non-coding base) gets the same `edit_file` and `search_files` grammar paragraphs, but skipped phase guidance. The grammar changes are universal — coding mode is just where we expect them to be used most.

### 6. Tool descriptions and frontends

`BUILTIN_TOOLS` ([`tools.rs:147`](../../crates/aictl-core/src/tools.rs)) — three description updates:

```rust
("read_file",   "read a file; optional --lines [N|N-M] for slice and numbered output"),
("edit_file",   "edit a file with multi-block find-and-replace; optional @start-end line scope and fuzzy fallback"),
("search_files","search file contents (ripgrep when available); --regex / --type / --context / --case"),
("find_files",  "find files by glob; --type for fast language filter (ripgrep when available)"),
```

The CLI's `/tools` command and the desktop Settings panel read from this constant directly, so no separate UI work.

### 7. Auto-detect `rg` once per process

```rust
static RG_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn rg_available() -> bool {
    *RG_AVAILABLE.get_or_init(|| {
        if std::env::var("AICTL_TEST_FORCE_RG_FALLBACK").is_ok() {
            return false;
        }
        std::process::Command::new("rg")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}
```

The probe runs at most once per session, on the first invocation of `search_files` or `find_files`. No startup-time cost. `AICTL_TEST_FORCE_RG_FALLBACK` exists for the fallback-path tests.

### 8. Runtime shape and integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/tools/filesystem.rs` | Extend `tool_edit_file` (multi-block + line scope + fuzzy); add `tool_search_files_rg` + retain `search_files_blocking` as fallback; same shape for `tool_find_files`; extend `tool_read_file` with `--lines` parsing; `RG_AVAILABLE` once-cell |
| `crates/aictl-core/src/tools.rs` | Update `BUILTIN_TOOLS` descriptions; no dispatch change |
| `crates/aictl-core/src/config.rs` | Revise `SYSTEM_PROMPT_CODING` (Explore + Code phase guidance, edit_file grammar block); revise `SYSTEM_PROMPT` edit/search paragraphs |
| `crates/aictl-core/Cargo.toml` | Add `regex` if not already direct (most likely present); optionally `globset` for the rg-output glob filter |
| `crates/aictl-cli/src/commands/tools.rs` | None — driven off `BUILTIN_TOOLS` |
| `crates/aictl-desktop/webview/src/components/Settings.tsx` | None — driven off the same data via the existing IPC |
| `README.md` | Bullet under "Coding-agent mode" calling out the smarter edit/search/read affordances |
| `CLAUDE.md` | One-line update next to the existing coding-mode paragraph: "Phase 2: smarter edit_file (multi-block, line-scope, fuzzy fallback), ripgrep-backed search/find when available, opt-in --lines for read_file." |
| `ROADMAP.md` | Remove the Phase 2 bullet once shipped |

### 9. Testing

**Unit tests** (in `tools/filesystem.rs`'s `#[cfg(test)]` mod):

- `parse_edit_body` table: single block, two blocks, block with `@start-end`, block with malformed `@N-` (missing end), block with line range but no `<<<`.
- `apply_edit_blocks`: clean two-block apply; abort-on-failure of second block; fuzzy fallback hit; fuzzy fallback ambiguity error.
- `read_file_lines` table: full file numbered, slice, single line, out-of-range clamp, zero-line error.
- `search_files_rg_args`: turn the input flag set into the exact `Vec<&str>` of rg args (pure transform, no subprocess).
- `find_files_filter`: rg `--files` simulated output filtered by glob.

**Integration tests** (CLI mock-LLM harness):

- A fixture that issues a two-block `edit_file` and verifies both changes hit disk.
- A fixture that triggers fuzzy fallback (whitespace-only diff between memory and disk).
- A fixture that issues `search_files --regex 'fn \w+'` and verifies regex semantics (only meaningful when rg is installed in the test runner — gate the assertion on `rg_available()`).
- A fixture that issues `read_file foo.rs\n--lines 10-20` and verifies the body is exactly the requested span with `NN: ` prefix.

**CI gates**

```bash
# `rg` availability must not be a build-time hard dep.
! grep -rE '"ripgrep"' crates/aictl-core/Cargo.toml
# Tool count is unchanged.
grep -E 'TOOL_COUNT: usize = ' crates/aictl-core/src/tools.rs | head -1
```

**Manual smoke**

1. In a Rust project, ask the agent to "rename `foo` to `bar` in `src/lib.rs` and `src/main.rs`" — verify it uses one `edit_file` per file with one block each (or one `edit_file` with two blocks, depending on the model). Confirm a clean diff.
2. Repeat with a deliberately stale model context: edit the file manually first, then ask the agent to apply a one-line change. Verify the fuzzy fallback succeeds when the difference is whitespace only.
3. Install rg, ask "find every use of `tokio::spawn` in this repo". Confirm rg ran (compare timing vs. `AICTL_TEST_FORCE_RG_FALLBACK=1`) and `.gitignore` was respected.
4. Uninstall rg (or set the env override). Same query still works, slower, no `.gitignore` filtering.
5. Ask the agent to "show me lines 120–160 of `src/run.rs` with line numbers" — verify the format and that the slice matches `sed -n '120,160p'`.

## Rollout phases

Phase 2 ships as one PR per sub-area (smarter `edit_file`, rg-backed search/find, selective read) — none of them block each other. Suggested order:

1. **Selective `read_file`** — smallest, lowest blast radius, immediately useful for the model.
2. **Smarter `edit_file`** — depends on (1) for the `@start-end` directive UX.
3. **Ripgrep-backed search/find** — independent, can land in parallel; same PR or two depending on review appetite.
4. **Prompt and doc sweep** — single commit once the three above land, so `SYSTEM_PROMPT_CODING` reflects the actual shipped grammar.

## Verification

Phase 2 sign-off requires:

1. `cargo build --workspace` clean, default features and `--all-features`.
2. `cargo lint` clean.
3. `cargo test` clean including the new unit + integration tests.
4. Existing Phase 1 tests pass without modification — the new tool grammars are strictly additive.
5. `BUILTIN_TOOLS` length still equals `TOOL_COUNT`.
6. Manual smoke checklist above.
7. The `--info` banner and `/tools` printer reflect the new descriptions.

## Risks

- **Two-block partial apply**: a bug in `apply_edit_blocks` could write a partially-updated file. Mitigation: read once, transform in-memory, write once. Add a property test that the file is byte-identical to the original when any block fails to find its target.
- **Fuzzy match wrong-anchor**: the whitespace-normalize fallback could match the wrong spot in a file with many similar shapes. Mitigation: require uniqueness *after* normalization, surface "N candidates" otherwise, and document that line-scope (`@start-end`) is the precise tool when the model already knows the location.
- **Ripgrep version drift**: rg `--type` lists differ slightly between versions (e.g. `rust` vs. `r` aliases). Mitigation: only document the language names from rg ≥ 13.0 (Debian stable's version); fall back gracefully when rg returns a "unknown type" error by re-running without the flag and noting the skip in the output.
- **`.gitignore` skips intended targets**: a user editing `target/debug/foo.rs` won't find it via search. Mitigation: document the `--no-ignore` flag; the default still matches conventional code-search expectations.
- **Prompt bloat**: the new grammar examples add tokens to every coding-mode turn. Mitigation: keep example fragments short (single `<<< === >>>` block, one `@10-20`, one `--lines 5-10`); measure token cost on a smoke run before merging.
- **Existing callers depending on flag-free `read_file` output**: a session or skill that pipes `read_file` straight into `edit_file` could break if line numbering became default. Mitigation: numbering is opt-in via `--lines`; today's bare-path input is unchanged.

## Scope boundaries with other plans

- **Phase 1 (`coding-agent.md`)**: prerequisite. The system-prompt seam already exists; we only add prose. The `WorkflowPhase` tracker is untouched.
- **Phase 3 (`coding-agent-phase-3.md`)**: orthogonal. The dedicated `test` tool and structured Review hook don't depend on these tool changes (but they benefit — Review can run a faster `search_files` to verify no stray TODO/FIXME left).
- **Phase 4 (`coding-agent-phase-4.md`)**: orthogonal. Parallel tool execution applies to the whole tool catalogue regardless of how rich any single tool is.

## Open questions

- **Default fuzzy on/off**: ship with fuzzy fallback always on, or behind a `AICTL_CODING_EDIT_FUZZY=true` flag? Lean "always on but only after an exact-match miss + whitespace-only diff" — the gating is structural, not behavioral, so a flag would mostly clutter config. Revisit if false-positive matches show up in dogfooding.
- **Regex default for `search_files`**: should the default be `--literal` (today's `contains` semantics) or `--regex`? Lean literal to avoid silently changing meaning for existing callers; the model can flip the flag when it wants regex.
- **`read_file --lines` and `edit_file` interaction**: when a slice is read with `--lines 10-20`, the model knows lines 10–20 exist. Should `edit_file`'s `@start-end` accept the same coordinates relative to a slice, or always absolute? Lean absolute (one coordinate system, no mode confusion); document in the prompt.
- **Ripgrep as a hard dep**: the fallback path keeps install-free working. Worth revisiting once we're confident every shipped platform has rg available — but that's a future call, not a Phase 2 one.
