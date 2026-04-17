# Plan: Plugin / Extension System for aictl

## Context

Users have domain-specific tool needs that don't belong in the core binary — a DevOps user wants `kubectl_query`, a data user wants `sql_query`, a security user wants `nmap_scan`, a research user wants `arxiv_search`. Today the only way to add a tool is to fork the repo, add a module under `src/tools/`, extend `execute_tool` dispatch, and rebuild. A plugin system lets users extend aictl without forking while keeping the core tool surface tight and the security model intact.

## Goals & Non-goals

**Goals**
- Let users add new tools as external artifacts (scripts first, WASM later).
- Route plugin calls through the existing `security::validate_tool()` pipeline so the CWD jail, disabled-tools list, and confirmation UX keep working.
- Expose plugin-declared schema to the LLM exactly like a built-in tool (same `<tool>name…</tool>` XML contract).
- Keep plugins opt-in and auditable — third-party code must not auto-load silently.

**Non-goals**
- No modifying the agent loop, slash commands, or LLM providers via plugins. One plugin = one tool.
- No privileged API into aictl internals (session history, keys, config). Plugins are pure input→output.
- No package registry / remote install in v1. Users drop files into `~/.aictl/plugins/` manually.
- No hot-reload. Plugins are discovered at startup; restart to pick up changes.

## Approach: Tiered Rollout

### Tier 1 — Script plugins (MVP)

An executable + manifest pair dropped into `~/.aictl/plugins/<name>/`. aictl spawns the executable under the existing subprocess sandbox, pipes input on stdin, reads result from stdout. Language-agnostic — any script/binary that reads stdin and writes stdout works.

### Tier 2 — WASM plugins (follow-up)

Single `.wasm` file + embedded manifest, executed via `wasmtime` with WASI capabilities explicitly granted in the manifest (filesystem/network denied by default). Safer and cross-platform, but heavier implementation — only worth doing if Tier 1 gets traction.

This plan focuses on Tier 1; Tier 2 is sketched at the end as a future phase.

---

## Tier 1 — Script Plugin Design

### 1. Layout on disk

```
~/.aictl/plugins/
├── kubectl_query/
│   ├── plugin.toml         # manifest
│   └── run                 # executable (any language; must be +x)
└── arxiv_search/
    ├── plugin.toml
    └── run.py
```

One directory per plugin. Directory name = plugin name (validated: alphanumeric + `_`/`-`, matching existing agent-name rules in `src/agents.rs`). The manifest is always `plugin.toml`; the entrypoint is whatever `manifest.entrypoint` points to (default `run`).

### 2. Manifest format (`plugin.toml`)

```toml
name = "kubectl_query"
version = "0.1.0"
description = "Query a Kubernetes cluster. Input: one line 'get|describe|logs <resource> [name]'."
entrypoint = "run"                       # relative to plugin dir
requires_confirmation = true             # default true; false only for read-only work
timeout_secs = 30                        # optional; falls back to global shell timeout
schema_hint = """
First line: subcommand (get|describe|logs)
Second line: resource type
Third line (optional): resource name
"""
```

Fields:
- `name` — must match directory name, re-validated at load.
- `description` — injected into the system prompt verbatim, like built-in tool descriptions.
- `entrypoint` — path inside plugin dir; resolved then canonicalized; must stay inside the plugin dir (symlink escape check).
- `requires_confirmation` — if `false`, plugin calls skip the interactive y/N gate. Still honored only when `--auto` would have skipped a built-in tool too. Defaults to `true`.
- `timeout_secs` — per-plugin override of the global shell timeout.
- `schema_hint` — free-form text appended to `description` in the tool catalog; the same convention used by other tools today.

### 3. Wire protocol

**Input** (stdin): the raw tool body exactly as the LLM emitted it between `<tool>…</tool>` tags. No JSON framing — keeps the protocol identical to how `exec_shell`, `run_code`, `json_query` are written today.

**Output** (stdout, UTF-8): the tool result string. Returned to the LLM verbatim after output sanitization (`<tool>` tag escaping already done for built-ins).

**Exit code**:
- `0` — success, stdout is the result.
- non-zero — surfaced as `[exit N] <stderr>` back to the LLM, matching the `lint_file` / `json_query` convention.

**stderr**: captured and only surfaced on non-zero exit, so chatty plugins don't pollute tool output.

### 4. Discovery & catalog

New `src/plugins.rs` module:

```rust
pub struct Plugin {
    pub name: String,
    pub dir: PathBuf,
    pub entrypoint: PathBuf,          // canonicalized, validated inside dir
    pub description: String,
    pub requires_confirmation: bool,
    pub timeout_secs: Option<u64>,
}

static PLUGINS: OnceLock<Vec<Plugin>> = OnceLock::new();

pub fn init() -> Result<()>;           // called from main.rs after security::init()
pub fn list() -> &'static [Plugin];
pub fn find(name: &str) -> Option<&'static Plugin>;
```

`init()` walks `~/.aictl/plugins/*/plugin.toml`, parses each, validates name + entrypoint, logs (not prints) any malformed plugin, and stores the survivors. A malformed plugin is skipped, not fatal — one bad plugin must not break aictl startup.

### 5. System-prompt injection

Today `build_system_prompt()` in `main.rs` concatenates the base prompt with the tool catalog. Extend it to append enabled plugins to the catalog in the same format as built-in tools:

```
### <plugin_name> (plugin)
<description>

<schema_hint>
```

The `(plugin)` suffix makes it visible to the LLM that this isn't a first-party tool — useful for the LLM's own judgment, and trivially inspectable via `/tools`.

### 6. Execution path

In `tools.rs::execute_tool`, after the built-in dispatch table fails to match the tool name:

```rust
if let Some(plugin) = plugins::find(name) {
    return execute_plugin(plugin, body).await;
}
```

`execute_plugin` does the work:
1. Spawn `tokio::process::Command::new(&plugin.entrypoint)` — **no shell**.
2. Apply `util::scrubbed_env()` (same helper `exec_shell`/`git`/`run_code` use).
3. `.current_dir(security::working_dir())` — pin to the CWD jail.
4. `.stdin(piped()).stdout(piped()).stderr(piped()).kill_on_drop(true)`.
5. Write `body` to stdin, close stdin.
6. Wrap the wait in `tokio::time::timeout(plugin.timeout_secs.unwrap_or(security::shell_timeout()))`.
7. Non-zero exit → format as `[exit N] <stderr>`.
8. Return the result through the normal output-sanitization path so `<tool>` injection is still neutralized.

Crucially, `security::validate_tool()` runs *before* this dispatch in the existing gate, so:
- `AICTL_SECURITY_DISABLED_TOOLS` can disable plugin names just like built-ins.
- The confirmation prompt fires before execution (unless `requires_confirmation = false` + `--auto`).
- `--unrestricted` bypasses it exactly as it does for built-ins.

### 7. Opt-in & gating

- New config key `AICTL_PLUGINS_ENABLED` (bool, default **`false`**). Plugins are third-party code; they must not load silently.
- `aictl --list-plugins` — non-interactive listing, prints name/description/location/status.
- `/plugins` REPL command — interactive menu: list, enable-all / disable-all, reload (re-run `init()`), show manifest.
- The welcome banner gains a `plugins: N enabled` line when plugins are on; nothing when off (keeps the banner quiet for users who don't use the feature).

### 8. Confirmation UX

Reuse the existing y/N prompt from the agent loop. The prompt copy changes slightly for plugins to make the provenance visible:

```
[plugin: kubectl_query] requires confirmation
  describe
  pod
  my-pod
Execute? [y/N]
```

`requires_confirmation = false` only skips the prompt when `--auto` is set, matching how `exec_shell` is gated today. A plugin cannot silently execute in interactive mode.

### 9. Namespace collisions

If a plugin name collides with a built-in tool, the built-in wins and the plugin is skipped with a warning logged during `init()`. The reserved-name list is just the built-in tool names already in `tools.rs`. No per-plugin prefix — keeps the LLM-facing surface simple.

### 10. Platform concerns

- **Executable bit**: on Unix, entrypoint must be executable; `init()` checks and skips with a warning otherwise.
- **Windows**: not a target today (aictl ships macOS/Linux binaries). Defer Windows-specific entrypoint resolution (`.exe`, `.ps1`) until there's demand.
- **Shebangs**: we invoke the entrypoint directly, so `#!/usr/bin/env python3` works the same as it would from a shell.

### 11. Config summary

```
AICTL_PLUGINS_ENABLED=false             # master switch; default off
AICTL_PLUGINS_DIR=~/.aictl/plugins      # override discovery root (for testing)
AICTL_PLUGINS_DISABLED=                 # comma-separated names to skip at init()
```

Per-plugin enable/disable lives in config rather than in the manifest — users shouldn't have to edit a third-party manifest to silence it.

### 12. Integration points

| File | Change |
|------|--------|
| `src/plugins.rs` | **New** — discovery, manifest parsing, `Plugin` struct, `execute_plugin` |
| `src/tools.rs` | Fall-through to `plugins::find()` in `execute_tool`; extend `/tools` listing |
| `src/main.rs` | `mod plugins`, `plugins::init()` after `security::init()`, append plugin catalog in `build_system_prompt()`, `--list-plugins` flag |
| `src/commands.rs` | New `/plugins` command + `src/commands/plugins.rs` submodule (menu: list/reload/show manifest) |
| `src/config.rs` | Add `AICTL_PLUGINS_ENABLED`, `AICTL_PLUGINS_DIR`, `AICTL_PLUGINS_DISABLED` readers |
| `src/ui.rs` | Welcome banner gets a `plugins: N enabled` line when on |
| `Cargo.toml` | Add `toml` crate for manifest parsing (already a common transitive dep; verify before adding) |

### 13. Testing

- **Unit tests** (`src/plugins.rs`):
  - Manifest parse: happy path, missing fields, name mismatch, entrypoint escape attempt (symlink pointing outside plugin dir), name-collision with built-in.
  - Catalog rendering: plugin tools appear with `(plugin)` suffix.
- **Integration tests** (new `tests/plugins.rs`, using a mock provider once `tests/` lands):
  - Plugin stdout → tool result round-trip.
  - Non-zero exit → `[exit N]` surface.
  - Timeout → child killed, error surfaced.
  - `security::validate_tool()` disabled-tools list blocks a plugin.
  - `AICTL_PLUGINS_ENABLED=false` hides plugins from the catalog and from dispatch.
- **Manual smoke test**: ship a 10-line `echo_back` example plugin in `examples/plugins/echo_back/` so users have a reference. Not installed automatically.

### 14. Documentation

- New `docs/plugins.md` (or a section in README.md — decide during implementation based on size): manifest reference, wire protocol, security model, example walkthrough.
- `ARCH.md` gains a "Plugins" section describing discovery, execution, and the security gate.
- `CLAUDE.md` gets a one-paragraph addition describing `src/plugins.rs`.
- `ROADMAP.md`: remove the "Plugin / extension system" entry from the Developer Experience section once Tier 1 ships.

---

## Tier 2 — WASM Plugins (future phase)

Sketch only; implement after Tier 1 has real users.

- Single `.wasm` file at `~/.aictl/plugins/<name>.wasm`; manifest embedded as a custom WASM section or sibling `.toml`.
- Execute via `wasmtime` with a WASI context built from the manifest's capability declarations (`fs_read`, `fs_write`, `net_connect`, each scoped to allowed paths/domains). Default = no capabilities.
- Same input (stdin bytes) / output (stdout bytes) contract as Tier 1, so plugin authors can share logic.
- Dependencies (`wasmtime` + `wasi-common`) add meaningful binary size — put behind a `plugins-wasm` cargo feature like `gguf`/`mlx` so users who don't want it don't pay for it.
- Security gains: WASM sandbox prevents filesystem/network access the user didn't explicitly grant, which Tier 1 can't offer (a script plugin runs with the user's full privileges, same as `exec_shell`).

## Rollout phases

1. **Phase 1** — `src/plugins.rs` + manifest parsing + script execution + `--list-plugins` flag. Gated behind `AICTL_PLUGINS_ENABLED`. No REPL surface yet.
2. **Phase 2** — `/plugins` REPL menu, welcome banner line, system-prompt catalog injection.
3. **Phase 3** — Integration tests, example plugin, `docs/plugins.md`.
4. **Phase 4** (optional, later) — Tier 2 WASM support behind a cargo feature.

## Verification

1. `cargo build` and `cargo build --release` — clean.
2. `cargo lint` (clippy pedantic per `.cargo/config.toml`) — no warnings.
3. `cargo test` — unit + integration tests pass.
4. Manual:
   - Drop the example `echo_back` plugin in place; confirm it shows up in `/tools` and `--list-plugins`.
   - Call it through a prompt and verify the round-trip.
   - Disable it via `AICTL_PLUGINS_DISABLED=echo_back` and confirm it disappears from the catalog.
   - Add it to `AICTL_SECURITY_DISABLED_TOOLS` and confirm dispatch is blocked at the security gate.
   - Write a plugin that sleeps past the timeout; confirm the child is reaped and a timeout error surfaces.
   - Write a plugin whose entrypoint symlinks to `/bin/sh`; confirm `init()` rejects it.
   - Start aictl with `AICTL_PLUGINS_ENABLED=false`; confirm no plugin tools appear anywhere.

## Open questions

- Should a plugin be able to declare it consumes/emits images (for vision workflows)? Deferred — start with text-only stdin/stdout; revisit when a concrete use case appears.
- Should per-plugin confirmation copy be customizable via the manifest (e.g., a `confirmation_hint` field)? Probably yes, but low priority — defer until a plugin author asks for it.
- Registry / `aictl plugin install <name>` — out of scope for v1. Manual copy is fine while the surface is small.
