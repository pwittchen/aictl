# Plan: Modular Architecture (Cargo workspace split)

## Context

Today `aictl` is a single binary crate (~47k LOC across 50+ modules). Engine code (agent loop, providers, security, sessions, tools) and frontend code (REPL, termimad markdown, spinners, rustyline) share one module tree and one dependency graph. The roadmap calls for a Server (HTTP gateway) and Desktop (Tauri) frontend, both of which would duplicate `run::run_agent_turn` wrapper logic and re-implement tool dispatch if we don't separate engine from presentation first.

This plan is a prerequisite for three other roadmap entries (Server, Desktop, and a future MCP-server-mode for aictl) and a long-term enabler for anyone who wants to embed the engine. It is deliberately **infrastructure-only** — zero user-visible behavior changes. Every `cargo test`, every REPL flow, every slash command, every `--flag` works identically before and after.

Scope-boundary statement up front: this plan covers **only** the workspace split and the trait-surface extraction. It does **not** cover writing `aictl-server` or `aictl-desktop` — those land in their own plans once `aictl-core` is stable.

## Goals & Non-goals

**Goals**
- Cargo workspace with `aictl-core` (lib), `aictl-cli` (bin). Future binaries (`aictl-server`, `aictl-desktop`) depend on `aictl-core` only.
- Every `println!`/`eprint!`/terminal-library call in a core module becomes a call on the `AgentUI` trait (or a typed event on a channel for non-UI frontends).
- `aictl-core` compiles without the terminal stack (`crossterm`, `rustyline`, `termimad`, `indicatif`).
- `aictl-core`'s public surface is explicit (`pub use`), documented, and treated as semver-stable from the moment the split lands.
- Feature flags (`gguf`, `mlx`, `redaction-ner`, and the future `mcp`) live on `aictl-core` because they gate provider/redaction code.
- CI green at every intermediate step — no multi-day broken-tree refactor.

**Non-goals**
- Not writing `aictl-server` or `aictl-desktop`. Those are separate plans, unblocked by this one.
- Not re-plumbing the agent loop's internals. `run_agent_turn` keeps the same shape; only its side-effect surface moves behind a trait.
- Not changing the conversation model (`Message`, `Role`, `ImageData`) — these are already provider-agnostic and stay in core.
- Not extracting a third "protocol" crate for wire types. Over-engineering for a two-frontend world; revisit if a third frontend appears.
- Not publishing to crates.io. `aictl-core` is a workspace-internal lib for now. Publishing is a v2 decision after the API has settled.
- Not reworking config, keyring, or session storage layout. `~/.aictl/` stays exactly as it is.
- Not splitting `tools/` by category or `llm/` by provider family. The existing submodule structure is fine.

## Current-state inventory

These numbers come from grepping the tree before writing this plan. They're the concrete debt the split has to pay down:

- **Modules with direct side-effect calls that need to route through a trait** (outside `commands/`, `ui.rs`, `repl.rs`, `main.rs`):
  - `src/config.rs` — 1 `eprintln!` (HOME missing warning).
  - `src/run.rs` — 3 `crossterm::` calls (raw-mode toggle for Esc-cancel listener).
  - `src/tools.rs` — 1 `eprint!` (the fallback y/N confirm prompt — already dead code on the InteractiveUI path).
  - `src/llm/gguf.rs` — 1 `indicatif::` (download progress bar).
  - `src/llm/mlx.rs` — 1 `indicatif::` (download progress bar).
  - `src/security/redaction.rs` — 6 `eprintln!` (NER init warnings, allow-pattern parse warnings).
  - `src/security/redaction/ner.rs` — 3 leak sites (1 `indicatif::`, 2 `eprintln!`).
  - `src/error.rs` — 2 (imports `rustyline::error::ReadlineError` in the `From` impl).
- **Process-global state** (stays where it is; documented as core-owned):
  - `config::CONFIG: OnceLock<RwLock<HashMap<...>>>`
  - `security::POLICY: OnceLock<SecurityPolicy>`
  - `session::CURRENT: Mutex<Option<Session>>`, `INCOGNITO: AtomicBool`
  - `tools::CALL_HISTORY: OnceLock<Mutex<HashSet<...>>>`
- **`std::env::current_dir()` in core** — 3 call sites in `security.rs` (default-policy builders). These already go through `SecurityPolicy.working_dir`, so the roadmap's "parameterize working directory" requirement is mostly satisfied; only the *construction* of the default policy reads CWD, and that can take the CWD as an argument.
- **Top-level modules** (all currently `mod X` in `main.rs`):
  - `agents`, `audit`, `commands`, `config`, `error`, `keys`, `llm`, `message`, `repl`, `run`, `security`, `session`, `skills`, `stats`, `tools`, `ui`, `version_cache`.

This is small. The bulk of `aictl` is already engine code that merely happens to live in the same crate as the CLI; the refactor is mostly **moving files and tightening a trait**, not rewriting logic. Estimating 3–4 focused days of work, not weeks.

## Target layout

```
Cargo.toml                       # workspace manifest
crates/
├── aictl-core/
│   ├── Cargo.toml               # lib crate; features: gguf, mlx, redaction-ner, (future) mcp
│   └── src/
│       ├── lib.rs               # re-exports the stable public surface
│       ├── agents.rs + agents/
│       ├── audit.rs
│       ├── config.rs
│       ├── error.rs
│       ├── keys.rs
│       ├── llm.rs + llm/
│       ├── message.rs
│       ├── run.rs
│       ├── security.rs + security/
│       ├── session.rs
│       ├── skills.rs + skills/
│       ├── stats.rs
│       ├── tools.rs + tools/
│       └── ui.rs                # AgentUI trait definition only (no impls)
└── aictl-cli/
    ├── Cargo.toml               # bin crate; depends on aictl-core
    └── src/
        ├── main.rs              # clap, startup wiring, REPL entry
        ├── commands.rs + commands/
        ├── repl.rs              # rustyline loop + tab completion
        ├── ui/
        │   ├── plain.rs         # PlainUI impl (single-shot)
        │   ├── interactive.rs   # InteractiveUI impl (REPL, termimad, crossterm)
        │   └── banner.rs        # welcome banner, status line
        ├── version_cache.rs
        └── integration_tests.rs
```

Why `crates/aictl-core` and `crates/aictl-cli` rather than `core/` and `cli/`: Cargo convention, matches popular workspaces (`rust-analyzer`, `nushell`, `polars`), and keeps the workspace root tidy if a third crate (`aictl-server`, etc.) joins later.

## The `AgentUI` trait after extraction

The existing trait (`src/ui.rs:95`) already covers most of what the agent loop needs. Extraction requires **adding** methods for the handful of side-effect call sites that still reach for `eprintln!` or a terminal library directly. The trait must stay small, event-shaped, and frontend-agnostic (no `crossterm` types in signatures).

Additions (with the call site that motivates each):

```rust
pub trait AgentUI {
    // --- existing (unchanged) ---
    fn show_reasoning(&self, text: &str);
    fn show_auto_tool(&self, tool_call: &ToolCall);
    fn show_tool_result(&self, result: &str);
    fn confirm_tool(&self, tool_call: &ToolCall) -> ToolApproval;
    fn show_answer(&self, text: &str);
    fn show_error(&self, text: &str);
    fn stream_begin(&self) {}
    fn stream_chunk(&self, _text: &str) {}
    fn stream_suspend(&self) {}
    fn stream_end(&self) {}
    fn show_token_usage(&self, ...);
    fn show_summary(&self, ...);

    // --- new ---
    /// Non-fatal warning (setup issue, deprecated config, skipped file, …).
    /// Replaces the scattered `eprintln!("Warning: …")` calls in
    /// `config.rs`, `security/redaction.rs`, `security/redaction/ner.rs`.
    fn warn(&self, text: &str);

    /// Long-running operation indicator. The engine opens one
    /// (`progress_begin`), pushes zero-or-more updates (`progress_update`),
    /// and closes it (`progress_end`). PlainUI no-ops; InteractiveUI maps
    /// to `indicatif`. Replaces the direct `indicatif::` usage in
    /// `llm/gguf.rs`, `llm/mlx.rs`, `security/redaction/ner.rs`.
    fn progress_begin(&self, label: &str, total: Option<u64>) -> ProgressHandle;
    fn progress_update(&self, handle: &ProgressHandle, current: u64, message: Option<&str>);
    fn progress_end(&self, handle: ProgressHandle, final_message: Option<&str>);

    /// Raw-mode key listener for Esc-to-cancel. The CLI's InteractiveUI
    /// wires this to `crossterm::event::poll`; server/desktop return a
    /// future that never resolves (no Esc channel in those contexts).
    /// Replaces the 3 `crossterm::` calls in `run.rs::with_esc_cancel`.
    fn interruption(&self) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub struct ProgressHandle(/* opaque; UI-owned */);
```

`ProgressHandle` is a newtype wrapping whatever the impl needs (for `indicatif`, a `ProgressBar`; for a no-op UI, `()`). Kept opaque so the trait signature doesn't leak `indicatif` into core.

The `interruption` method is the tricky one — `run.rs::with_esc_cancel` currently enables raw mode and spawns a blocking poller. After the move, the same logic lives in `InteractiveUI` and the trait surfaces only the `Future` that resolves when the user cancels. `PlainUI` and server/desktop UIs return a pending future. The engine's agent loop gets simpler too: `select!` over the provider call and the UI's interruption future.

Event-channel alternative considered: collapse the trait into a single `Sender<AgentEvent>` enum. Rejected for v1 — the CLI needs synchronous `confirm_tool` (returns the approval) and synchronous `progress_*` handles, both of which are awkward over a one-way channel. The channel shape makes sense for `aictl-server`'s HttpUI, which will build a thin adapter from `AgentUI` calls onto an `mpsc::Sender<AgentEvent>` internally. One trait, two styles of consumer.

## Migration strategy

The cardinal rule: every commit leaves the tree green under `cargo build && cargo lint && cargo test`. We never push a broken state and fix it later. The only way to pull this off with confidence is to do the refactor in explicit, small, independently-mergeable phases.

### Phase 0 — Inventory commit (no code changes)

Write a throw-away `ARCH.md` appendix enumerating every side-effect call site we plan to relocate, with file/line references. Useful as a checklist during Phase 2 and as PR-review bait ("did you miss any?"). Tiny commit; the appendix is deleted when the refactor is done.

### Phase 1 — Extend `AgentUI` in-place, without moving files

Add `warn`, `progress_*`, and `interruption` to the existing `src/ui.rs` trait. Implement them in `PlainUI` and `InteractiveUI`. Replace every identified call site in core modules with the trait call, threading `&dyn AgentUI` through the function signatures that need it.

Call-site fixes, in order:

1. `config.rs::home_dir()` — currently `eprintln!` on missing HOME. Two options:
   - (a) Return `Result<PathBuf, String>` and let the caller decide (`main.rs` surfaces via `ui.warn()`).
   - (b) Add a one-shot `config::set_warning_sink(Box<dyn Fn(&str) + Send + Sync>)` that the CLI wires up at startup.
   Prefer (a) — returning the error is more idiomatic than a global sink, and the call is only made during init.
2. `security/redaction.rs` and `security/redaction/ner.rs` — the NER init path already returns a `NerStatus` enum; the `eprintln!`s sit *inside* the match that consumes it. Push them one level up, into the call site in `main.rs`, where `&dyn AgentUI` is already available. No API change; one more `match` in `main.rs`.
3. `llm/gguf.rs` and `llm/mlx.rs` — download progress. These functions already take a `TokenSink` for streaming; pass `&dyn AgentUI` too, or a narrower `ProgressSink` trait (subset of `AgentUI`). I'd pass `&dyn AgentUI` — one trait is easier to reason about than two.
4. `run.rs::with_esc_cancel` — split into a generic `with_cancel<F>(f, interruption: Pin<Box<...>>)` in core, plus the raw-mode `crossterm` implementation living in `InteractiveUI::interruption`. Tests already stub Esc cancel under `#[cfg(test)]`, so the stub becomes the "server/desktop returns pending" path naturally.
5. `tools.rs::confirm_tool_call` — already dead code, superseded by `InteractiveUI::confirm_tool`. Delete.

Phase 1 output: the tree still builds as one crate, but no core module contains `println!`/`eprintln!`/`crossterm::`/`indicatif::`/`rustyline::`. A single grep at the end of Phase 1 proves this:

```bash
grep -rE 'println!|eprintln!|print!\(|eprint!\(|termimad::|indicatif::|crossterm::|rustyline::' \
    src/config.rs src/error.rs src/llm.rs src/llm/ src/run.rs src/security.rs src/security/ \
    src/session.rs src/tools.rs src/tools/ src/agents.rs src/agents/ src/skills.rs src/skills/ \
    src/audit.rs src/stats.rs src/keys.rs src/message.rs
```

Must return empty.

This phase is mergeable as one or two PRs on its own. It delivers value even if we never finish the workspace split: future frontends just reuse the trait work.

### Phase 2 — Introduce the workspace, CLI still one crate

Create `Cargo.toml` at the root as a workspace manifest. Move the existing `src/` tree and `Cargo.toml` under `crates/aictl-cli/` with a single `git mv` (preserving history). `Cargo.toml` at the root becomes:

```toml
[workspace]
members = ["crates/aictl-cli"]
resolver = "3"
```

No crate split yet. `cargo build` still produces one binary. Goal of this phase: isolate the workspace conversion from the logic split so bisecting later is easier. CI runs against `cargo build --workspace`. `cargo fmt` / `cargo lint` aliases in `.cargo/config.toml` keep working because they operate on whatever package the current directory resolves to.

One PR. Mechanical.

### Phase 3 — Birth `aictl-core`, start moving modules

Create `crates/aictl-core/` with a skeleton `Cargo.toml` and an empty `lib.rs`. Add it to the workspace. `aictl-cli` takes a path dependency on it:

```toml
# crates/aictl-cli/Cargo.toml
[dependencies]
aictl-core = { path = "../aictl-core" }
```

Now we iteratively move modules from `aictl-cli/src/` to `aictl-core/src/`. Order is driven by dependency direction:

1. **Leaves first**: `message`, `error`, `keys`, `config`, `stats`, `audit`. Zero inbound deps on frontend code. Move, re-export from `aictl-core::lib`, replace `crate::...` with `aictl_core::...` at the call sites in `aictl-cli`.
2. **Security**: `security.rs` + `security/`. Depends only on `config`. Same drill.
3. **Tools**: `tools.rs` + `tools/`. Depends on `security`, `audit`, `config`, `message`. The `crate::ImageData` import at `tools.rs:19` needs to become `aictl_core::message::ImageData`. No behavior change.
4. **Session + agents + skills**: each self-contained modulo its remote submodule. Move whole, including `agents/remote.rs` and `skills/remote.rs`.
5. **LLM**: `llm.rs` + `llm/`. Whole subtree. The provider-call functions already take `TokenSink` and `&dyn AgentUI` after Phase 1, so there's no coupling left to the CLI.
6. **Run**: `run.rs`. Last, because it pulls in everything above.
7. **UI trait**: move the `trait AgentUI` definition (and the `ToolApproval`, `ProgressHandle` types) from `ui.rs` into `aictl-core/src/ui.rs`. The implementations (`PlainUI`, `InteractiveUI`) stay in `aictl-cli/src/ui/`.

After each step: `cargo build --workspace && cargo lint && cargo test`. If any step is too big to keep green in one commit (LLM subtree is the likeliest), split it further — one provider module at a time, keeping stubs in the CLI side for anything not yet moved.

Each module's move is one PR. Seven PRs, possibly nine if LLM needs splitting. All are mechanical file-moves + `use`-path rewrites; no logic changes.

### Phase 4 — Lock the public API

`aictl-core/src/lib.rs` gains an explicit re-export block:

```rust
pub use agents::{Agent, AgentMeta, load_agent, list_agents /* ... */};
pub use audit::{log_tool, Outcome /* ... */};
pub use config::{config_get, config_set, config_unset /* ... */};
// ...
```

Everything not explicitly re-exported is `pub(crate)` or private. Run `cargo doc --no-deps --package aictl-core` and check the surface by eye. Anything surprising either gets hidden or promoted to documented.

The existing `pub(crate)` visibility on `run::run_agent_turn`, `TurnResult`, `Provider`, etc., is upgraded to `pub` — these are legitimately the core's API. `pub(crate)` semantics in a lib crate are less useful than in a bin crate; re-audit each one.

### Phase 5 — Feature flag alignment

`gguf`, `mlx`, `redaction-ner` move from `aictl-cli/Cargo.toml` to `aictl-core/Cargo.toml`. `aictl-cli` declares them as passthroughs:

```toml
# crates/aictl-cli/Cargo.toml
[features]
default = []
gguf = ["aictl-core/gguf"]
mlx = ["aictl-core/mlx"]
redaction-ner = ["aictl-core/redaction-ner"]
```

User-visible behavior unchanged: `cargo build --features gguf` still works from the CLI crate directory or the workspace root. The future `mcp` feature (per the MCP plan) lands on `aictl-core` directly.

Dev-dependencies (`tempfile`) stay per-crate.

### Phase 6 — Documentation and CI hygiene

- `README.md` gets a short "workspace layout" section.
- `ARCH.md` replaces its current module map with the workspace structure; the "Why this comes first" paragraph in the roadmap moves here as permanent reference.
- `CLAUDE.md` updates module paths and the build-and-run section (`cargo build` at the workspace root builds both crates; `cargo run --bin aictl -- …` replaces `cargo run -- …`).
- `.cargo/config.toml` aliases: `cargo lint` and `cargo fmt` keep working at workspace root.
- GitHub Actions: run `cargo build --workspace`, `cargo lint --workspace`, `cargo test --workspace`. Matrix stays the same.
- Release packaging (if any `cargo-dist` / goreleaser config exists — verify during Phase 6) points at `aictl-cli` explicitly.

## Risks

- **Hidden coupling**: a core module might rely on a `Cargo.toml`-level dep that the CLI currently carries. `cargo build` will catch this, but it may force an unexpected dep move (e.g., `scraper` is listed at CLI level but used only by `tools/web.rs`). Mitigation: for each module move in Phase 3, audit the dep list before moving.
- **Feature-gated test builds**: `mlx`/`gguf` aren't enabled in default CI. A mis-gated `cfg` could compile only with a feature on. Mitigation: run `cargo check --workspace --all-features` at the end of Phase 5.
- **Agent-loop `interruption()` signature**: the `Pin<Box<dyn Future<Output=()> + Send>>` return type isn't the most elegant Rust. Alternatives (`async fn` in trait; associated type) have their own issues (`async fn` in traits is still young; associated type leaks the concrete future type out of the trait). Worth accepting the `Box<dyn Future>` cost for stability; revisit when `async fn` in traits is battle-tested.
- **Static config/security/session state crosses crate boundary**: fine in Rust — `static` items are per-process, not per-crate. But a consumer who spawns a second engine in the same process gets a second agent loop sharing the same `CONFIG` / `POLICY`. Call out in docs; revisit if a consumer actually wants multi-tenancy (most likely server). For v1 it's single-tenant-per-process.
- **Path history in `git log`**: files moved under `crates/aictl-cli/` and then again to `crates/aictl-core/` will need `git log --follow` to trace. Not a blocker; standard workspace-split pain. One consolidated-move commit per module minimizes noise.
- **Contributors in flight**: during the refactor window, open PRs will conflict. Communicate timing; do the workspace conversion (Phase 2) and the bulk of Phase 3 in one weekend to shrink the conflict window.

## Scope boundaries with other plans

- **Server plan** (to be written): consumes `aictl-core` only. Starts after Phase 5 lands. Builds `HttpUI` against the `AgentUI` trait.
- **Desktop plan** (to be written): same as Server. Tauri shell + `DesktopUI`.
- **MCP plan** (`.claude/plans/mcp-support.md`): client-side MCP support lands in `aictl-core` (under the `mcp` feature), reachable by any frontend. The plan's `mcp::init()` runs in `main.rs`-equivalent for each binary.
- **Plugin plan** (`.claude/plans/plugin-system.md`): same — plugins live in `aictl-core`, discovered at startup by whatever frontend is running.
- **IPC plan** (`.claude/plans/inter-instance-messaging.md`): REPL-only. Lives in `aictl-cli` because `IpcEndpoint` is coupled to the interactive loop. Server/desktop ignore it.

## Testing strategy

- **Existing test suite** must pass at every phase. This is the primary signal.
- **Crate-boundary test** after Phase 3: write a tiny `aictl-core`-only integration test in `crates/aictl-core/tests/smoke.rs` that spins up the mock LLM provider, runs one turn, and asserts the answer. Depends on nothing from `aictl-cli`. Proves the engine stands alone.
- **Public-API snapshot**: add `cargo-semver-checks` or `cargo-public-api` as a dev-tool (not a CI gate initially) to catch unintended public-surface changes going forward. Worth adding once, even if only run manually.
- **Feature matrix**: one-off CI job post-Phase 5 that runs `cargo build --workspace --all-features` and `cargo build --workspace` (no features). The existing per-feature jobs cover everything else.

## Verification (per phase)

| Phase | Build | Lint | Test | Additional |
|-------|-------|------|------|------------|
| 0 | n/a | n/a | n/a | ARCH appendix merged |
| 1 | `cargo build` | `cargo lint` | `cargo test` | grep for leaks returns empty |
| 2 | `cargo build --workspace` | `cargo lint --workspace` | `cargo test --workspace` | `aictl` binary runs end-to-end |
| 3 (per module move) | same | same | same | `aictl-core` smoke test passes |
| 4 | same | same | same | `cargo doc --no-deps` clean |
| 5 | `cargo build --workspace --all-features` | same | same | feature builds each work from both crate dirs |
| 6 | same | same | same | README/ARCH/CLAUDE updated, release build artifact still named `aictl` |

Final sign-off requires:
1. `cargo build --workspace` clean on default features and `--all-features`.
2. `cargo lint --workspace` clean.
3. `cargo test --workspace` clean on default features and `--all-features`.
4. Grep for forbidden symbols in `aictl-core`:
   ```bash
   grep -rE 'println!|eprintln!|print!\(|eprint!\(|termimad::|indicatif::|crossterm::|rustyline::|dialoguer::' \
       crates/aictl-core/src/
   ```
   Returns empty.
5. `crates/aictl-core/Cargo.toml` does **not** list `crossterm`, `rustyline`, `termimad`, `indicatif` as dependencies.
6. Manual smoke in a fresh checkout: clone, `cargo install --path crates/aictl-cli`, run a REPL turn, run `--list-agents`, run `--message`, confirm identical output to pre-refactor `master`.

## Open questions

- **Do we need a `aictl-types` crate** for `Message`, `Role`, `ImageData`, `TokenUsage`, and event enums — so a future JS/TS client could use them via `ts-rs` or `specta`? Deferred. The Desktop plan (Tauri-based, Rust-backed) doesn't need it; a hypothetical web client would, but that's far enough away to not block this work. If we later add it, `aictl-core` depends on `aictl-types`, frontends re-export, no disruption.
- **Is `run::Provider` a public-API type?** It's an enum of backend providers, used by the CLI to select a code path. Leaning yes — it's the clearest way to tell the engine which provider to use. If we later want to hide it behind a builder, we can add the builder and deprecate direct `Provider` use without breaking consumers too badly.
- **Should the Esc-cancel `interruption` method be optional?** The server/desktop never need it. We could split `AgentUI` into `AgentUI` (required: show_*, confirm_tool, stream_*) and `Interactive: AgentUI` (adds `interruption`). Probably overkill — default `async { std::future::pending().await }` for non-interactive UIs is fine.
- **Workspace `resolver = "3"`**: the CLI is on edition 2024, which implies resolver 3. Confirm the workspace manifest sets it explicitly; Cargo warns otherwise.
- **Binary name stability**: `cargo install aictl` currently installs the one binary. Post-split, `aictl-cli` produces the binary named `aictl` (set via `[[bin]] name = "aictl"`). Double-check this before releasing.
- **Should we release a 0.x `aictl-core` to crates.io on the back of this?** No. Too early. The API will move as the Server and Desktop plans materialize. Keep it workspace-internal until at least one external consumer lands.
