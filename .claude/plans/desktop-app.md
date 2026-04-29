# `aictl-desktop` — development plan

A native desktop frontend for `aictl` that mirrors the CLI's behaviour on top of the existing `aictl-core` crate. macOS-only at this stage; cross-platform is a post-MVP concern.

This plan operationalizes the **Desktop** section of [`ROADMAP.md`](../../ROADMAP.md) — once the desktop ships, that roadmap entry comes out.

---

## 1. Goals & non-goals

### Goals

- Ship `aictl-desktop` as a third workspace crate, alongside `aictl-cli` and `aictl-server`.
- macOS-only binary in v1 (built and signed as a `.app` bundle and DMG).
- 100% reuse of `aictl-core`: agent loop, providers, tools, security gate, redaction, audit log, sessions, agents, skills, MCP, plugins, hooks, stats. No business logic forks.
- **Shared config with the CLI** — both binaries read and write `~/.aictl/config`, `~/.aictl/sessions/`, `~/.aictl/agents/`, `~/.aictl/skills/`, `~/.aictl/audit/`, `~/.aictl/stats`, `~/.aictl/mcp.json`, `~/.aictl/hooks.json`, `~/.aictl/plugins/`. A change made in one is immediately visible in the other.
- Visual identity matches `website/DESIGN.md` — dark brutalist, GeistMono + Inter, cyan accent, sharp corners, opacity-driven depth.

### Non-goals (v1)

- Linux / Windows builds. Code stays platform-agnostic where free, but distribution targets macOS only.
- A separate "desktop config". We do not introduce GUI-only settings files.
- Reimplementing CLI-only ergonomics that don't fit a windowed app (see §3 for the dropped surface).
- Code signing & notarization automation in CI — manual signing for the first release; CI signing is a follow-up.

---

## 2. Framework choice — Tauri v2

The roadmap prescribes Tauri v2; this plan accepts that.

**Why Tauri (not egui / iced / SwiftUI):**

- Chat UIs are a solved problem in HTML/CSS — markdown rendering, syntax highlighting, streaming text, scrollback, virtualized lists all exist as battle-tested web libs.
- Rust backend is `aictl-core` directly via `#[tauri::command]` wrappers — no FFI bridge, no second language for business logic.
- The same frontend bundle can later be reused for a hosted web UI on top of `aictl-server`.
- ~5–10 MB final binary; `WKWebView` is shipped with macOS so no Chromium download.
- Cross-platform escape hatch is preserved for v2.

**Frontend stack:**

- **Framework**: Solid.js. Small (~7 KB), fine-grained reactivity matches streamed token rendering well, no virtual-DOM overhead. (Svelte is the alternate; React is fine but heavier and the v18+ concurrent renderer is overkill here.)
- **Bundler**: Vite (Tauri's default).
- **Styling**: Hand-written CSS, mirroring `website/style.css` — no Tailwind, no component libs. Keeps the bundle small and the design tight.
- **Markdown**: `markdown-it` + `highlight.js` for code fences; KaTeX is out of scope for v1.
- **Icons**: Heroicons inline SVG (matches the website / DESIGN.md note).

---

## 3. Feature scope — what's in and what's dropped

The CLI exposes ~30 slash commands and ~25 CLI flags. The desktop is not a slash-command-shaped app: most surfaces become menu items, sidebar panels, modal dialogs, or window-chrome controls. A few surfaces don't translate at all and are deliberately omitted.

### Carry over (UI redesigned)

| CLI surface | Desktop equivalent |
|---|---|
| REPL chat loop | Main chat window — message list + composer |
| `/agent` + `--list-agents` + `--pull-agent` | Sidebar tab "Agents": list (local/global/official badges), browse remote, install, edit, delete |
| `/skills` + `--list-skills` + `--pull-skill` | Sidebar tab "Skills"; user-defined skills surface as `/<name>` slash commands inside the composer |
| `/session` + `--list-sessions` + `--clear-sessions` + `--incognito` | Sidebar tab "Sessions": list, search, rename, delete, "New incognito window" menu item |
| `/model` + `/keys` + `/balance` + `--list-balances` | Settings → Provider tab: provider/model picker, key entry (keyring-backed), balance probe button |
| `/config` + `/behavior` + `/security` + `/memory` | Settings → General / Security / Memory tabs (forms backed by `config_set`/`config_unset`) |
| `/hooks` + `--list-hooks` + `--test-hook` | Settings → Hooks tab: enable/disable, edit `hooks.json`, test runner with payload preview |
| `/mcp` + `--list-mcp` + `--mcp-server` | Settings → MCP tab: list servers, enable/disable, view tool catalogue, restart |
| `/plugins` + `--list-plugins` | Settings → Plugins tab: list, enable/disable |
| `/tools` | Settings → Tools tab: catalogue with descriptions, per-tool deny list |
| `/stats` | Sidebar tab "Stats": tokens, costs, per-provider breakdown |
| `/info` | About window |
| `/history` | Inline in the chat (the message list is the history); also a "Copy" button per message |
| `/copy` | "Copy" button on each assistant message + global ⌘⇧C copies last answer |
| `/clear` + `/compact` + `/undo` + `/retry` | Toolbar buttons in the chat header |
| `/gguf` + `/mlx` | Settings → Local Models tab: model browser, download progress, set default (only when the corresponding cargo feature is enabled at build time) |
| `/help` | Help menu → keyboard shortcut overlay (⌘/) |
| `/version` + `--update` | About window shows version + "Check for updates" — auto-update via Tauri's updater (Sparkle-style) |
| Esc to interrupt | Toolbar **Stop** button (red) while a turn is running; ⌘. global shortcut |
| Tool confirmation y/N | Modal sheet attached to the chat window with Allow / Deny / Always-allow buttons |
| `--auto` / unrestricted | Settings → Security: "Auto-approve tool calls" toggle (per-window override via toolbar) |
| `--quiet` | N/A (no piping) |
| Tool approval auto-accept-once | "Always allow this tool" checkbox in the approval modal |

### Dropped (don't translate)

- `--message` single-shot — every chat send is already a single turn; a separate flag adds nothing.
- `--audit-file` — the audit log lives at its session-keyed default path; advanced users open it from Finder. (Settings → Security exposes a "Reveal audit log in Finder" button.)
- `/ping` — useful in a terminal session, but a desktop user can simply send a test message; the latency bar can show round-trip ms instead.
- `/uninstall` — macOS apps are uninstalled by dragging to the Trash; we don't reinvent that.
- `/roadmap` — not user-facing on a polished GUI.
- `--prompt-file` flag — `AICTL.md` discovery still works; a per-window project-prompt picker is overkill for v1.
- The `--unrestricted` flag stays available as a build-time / preferences toggle, but is **not** a quick toggle (matches the CLI: it's deliberate, not casual).
- Pipe / stdin mode — desktop apps don't pipe.
- `loop` / streaming-token-to-stdout helpers — replaced by reactive Solid components.

### Deferred to v2

- Multi-window (a window per session). v1 has one chat window; switching sessions swaps content.
- System tray / menu-bar app.
- Native macOS share extension and "Open with aictl" Finder integration.
- Drag-and-drop of files into the composer (the `read_file` tool already covers this; a UX nicety, not core).
- Multi-pane diff viewer for the future "smarter edit tool" coding-agent feature.

---

## 4. Workspace & code layout

### New crate

```
crates/aictl-desktop/
├── Cargo.toml                 # depends on aictl-core; macos cfg-gates
├── tauri.conf.json            # Tauri v2 app config (bundle id, identifier, etc.)
├── build.rs                   # Tauri build hook
├── icons/                     # .icns + variants
└── src/
    ├── main.rs                # Tauri entry; cfg(target_os = "macos")
    ├── ui.rs                  # `DesktopUI` impl of `aictl_core::AgentUI`
    ├── events.rs              # event payload types serialized to the webview
    ├── commands/
    │   ├── mod.rs
    │   ├── chat.rs            # send_message, stop_turn, retry, undo, clear, compact
    │   ├── sessions.rs        # list/load/save/rename/delete/incognito
    │   ├── agents.rs          # list/install/delete + remote browse
    │   ├── skills.rs          # ditto
    │   ├── settings.rs        # get/set config, keys, hooks, mcp, plugins
    │   ├── tools.rs           # catalogue + tool-approval response
    │   ├── stats.rs
    │   ├── balance.rs
    │   ├── models.rs          # provider/model list, gguf/mlx browse
    │   └── system.rs          # version, update check, reveal audit log
    └── webview/               # frontend source
        ├── index.html
        ├── package.json
        ├── vite.config.ts
        ├── src/
        │   ├── App.tsx
        │   ├── main.tsx
        │   ├── styles/
        │   │   ├── tokens.css        # mirrors website design tokens
        │   │   └── components.css
        │   ├── lib/
        │   │   ├── ipc.ts            # typed wrappers around invoke()/listen()
        │   │   └── markdown.ts       # markdown-it + highlight.js
        │   └── components/
        │       ├── Chat/
        │       ├── Composer/
        │       ├── ToolApproval/
        │       ├── Sidebar/
        │       ├── Settings/
        │       └── Toolbar/
        └── public/                  # static assets
```

### `Cargo.toml` essentials

```toml
[package]
name = "aictl-desktop"
description = "Native desktop frontend for aictl (macOS)"
version.workspace = true
edition.workspace = true
repository.workspace = true
authors.workspace = true
license-file.workspace = true

[[bin]]
name = "aictl-desktop"
path = "src/main.rs"

[dependencies]
aictl-core = { path = "../aictl-core" }
tauri = { version = "2", features = ["macos-private-api"] }
tauri-plugin-dialog = "2"
tauri-plugin-clipboard-manager = "2"
tauri-plugin-shell = "2"
tauri-plugin-updater = "2"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[build-dependencies]
tauri-build = "2"

[features]
default = []
gguf = ["aictl-core/gguf"]
mlx = ["aictl-core/mlx"]
redaction-ner = ["aictl-core/redaction-ner"]
```

`Cargo.toml` (workspace root) `members` array gains `crates/aictl-desktop`.

`build.rs` and the macOS-only guard at the top of `main.rs`:

```rust
#![cfg(target_os = "macos")]
fn main() { ... }
```

CI gates the desktop build on `runner.os == 'macOS'`. Linux and Windows runners skip the crate.

---

## 5. Core API stabilization (work in `aictl-core`)

The desktop uncovers two API gaps in the core:

### 5.1 Channel-based event stream

The CLI's `AgentUI` is synchronous and shaped for stdout. A desktop frontend wants events delivered to the webview as they happen. We introduce a thin event enum and a sender that the `DesktopUI` impl owns.

```rust
// new: crates/aictl-core/src/ui/events.rs (re-exported from ui.rs)

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    SpinnerStart { message: String },
    SpinnerStop,
    Reasoning  { text: String },
    StreamBegin,
    StreamChunk { text: String },
    StreamSuspend,
    StreamEnd,
    ToolAuto    { tool: String, body: String },
    ToolResult  { text: String },
    Answer      { text: String },
    Error       { text: String },
    Warning     { text: String },
    TokenUsage  { /* fields from existing show_token_usage args */ },
    Summary     { /* fields from existing show_summary args */ },
    ProgressBegin  { id: u64, label: String, total: Option<u64> },
    ProgressUpdate { id: u64, current: u64, message: Option<String> },
    ProgressEnd    { id: u64, message: Option<String> },
}
```

This enum lives in `aictl-core` because the CLI may eventually consume it too (e.g., for testing). The CLI's `PlainUI`/`InteractiveUI` continue to render synchronously and don't depend on the enum.

### 5.2 Tool approval as `async` channel

Today `AgentUI::confirm_tool` returns `ToolApproval` synchronously. That works for the terminal because the prompt blocks. For the desktop, we need an `async` channel: the agent loop sends an approval request, awaits, and the webview eventually responds.

Two options:

**A. Add an async sibling method to the trait, keep the sync one**

```rust
pub trait AgentUI {
    fn confirm_tool(&self, tool_call: &ToolCall) -> ToolApproval;

    fn confirm_tool_async<'a>(&'a self, tc: &'a ToolCall)
        -> Pin<Box<dyn Future<Output = ToolApproval> + Send + 'a>>
    {
        // Default impl just defers to the sync version, so existing UIs
        // (PlainUI, InteractiveUI) keep working unchanged.
        let result = self.confirm_tool(tc);
        Box::pin(async move { result })
    }
}
```

The agent loop calls `confirm_tool_async` instead of `confirm_tool`. The CLI takes the default; the desktop overrides.

**B. Convert `confirm_tool` itself to async**

Cleaner long-term but a wider blast radius — every `AgentUI` impl and every site that holds an `&dyn AgentUI` becomes part of the diff.

**Decision: option A** for v1. Migrate to option B as a separate cleanup once the desktop ships.

The agent loop site (`crates/aictl-core/src/run.rs`, search `confirm_tool`) gets a one-line change: `ui.confirm_tool(&tc)` → `ui.confirm_tool_async(&tc).await`.

### 5.3 Misc

- `aictl_core::ui::warn_global` already exists — `DesktopUI` installs a sink that pushes a `Warning` event.
- `audit::set_file_override` is not needed — the desktop uses session-keyed audit files.
- A small helper, `aictl_core::run::run_agent_session`, is added to encapsulate "drive the agent loop on one user message, with this UI, this session, this provider"; both the CLI's `repl::run_and_display_turn` and the desktop's `chat::send_message` collapse onto it. (Nice-to-have, not blocking.)

---

## 6. `DesktopUI` implementation

```rust
// crates/aictl-desktop/src/ui.rs

use aictl_core::{AgentUI, ToolApproval, ProgressHandle};
use aictl_core::ui::events::AgentEvent;

pub struct DesktopUI {
    app: tauri::AppHandle,
    /// Outstanding tool-approval requests keyed by request id. The
    /// webview responds via the `tool_approval_response` command and
    /// resolves the oneshot.
    pending_approvals: Mutex<HashMap<u64, oneshot::Sender<ToolApproval>>>,
    next_request_id: AtomicU64,
}

impl AgentUI for DesktopUI {
    fn start_spinner(&self, msg: &str)         { self.emit(AgentEvent::SpinnerStart { message: msg.into() }); }
    fn stop_spinner(&self)                     { self.emit(AgentEvent::SpinnerStop); }
    fn show_reasoning(&self, text: &str)       { self.emit(AgentEvent::Reasoning { text: text.into() }); }
    fn stream_begin(&self)                     { self.emit(AgentEvent::StreamBegin); }
    fn stream_chunk(&self, text: &str)         { self.emit(AgentEvent::StreamChunk { text: text.into() }); }
    fn stream_suspend(&self)                   { self.emit(AgentEvent::StreamSuspend); }
    fn stream_end(&self)                       { self.emit(AgentEvent::StreamEnd); }
    fn show_auto_tool(&self, tc: &ToolCall)    { self.emit(AgentEvent::ToolAuto { tool: tc.name.clone(), body: tc.body.clone() }); }
    fn show_tool_result(&self, result: &str)   { self.emit(AgentEvent::ToolResult { text: result.into() }); }
    fn show_answer(&self, text: &str)          { self.emit(AgentEvent::Answer { text: text.into() }); }
    fn show_error(&self, text: &str)           { self.emit(AgentEvent::Error { text: text.into() }); }
    fn warn(&self, text: &str)                 { self.emit(AgentEvent::Warning { text: text.into() }); }

    fn confirm_tool(&self, _tc: &ToolCall) -> ToolApproval {
        // Should never be called directly — the loop uses confirm_tool_async.
        // If something does fall back here, deny safely.
        ToolApproval::Deny
    }

    fn confirm_tool_async<'a>(&'a self, tc: &'a ToolCall)
        -> Pin<Box<dyn Future<Output = ToolApproval> + Send + 'a>>
    {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending_approvals.lock().unwrap().insert(id, tx);
        self.emit(AgentEvent::ToolApprovalRequest { id, tool: tc.name.clone(), body: tc.body.clone() });
        Box::pin(async move {
            rx.await.unwrap_or(ToolApproval::Deny)
        })
    }

    fn show_token_usage(...)  { self.emit(AgentEvent::TokenUsage { ... }); }
    fn show_summary(...)      { self.emit(AgentEvent::Summary { ... }); }

    fn progress_begin(&self, label: &str, total: Option<u64>) -> ProgressHandle { ... }
}
```

The progress backend is a small struct that holds the `AppHandle` and the progress id, so the engine's existing `progress_update` / `progress_end` calls fan out to the webview.

Esc-cancellation (`AgentUI::interruption`) defaults to `pending` — the desktop uses an explicit `stop_turn` Tauri command and a tokio `CancellationToken` per turn instead of the raw-mode key listener.

---

## 7. IPC contract — Tauri commands and events

### 7.1 Commands (webview → Rust)

All commands return `Result<T, String>`. The frontend wraps each in a typed helper in `lib/ipc.ts`.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `send_message` | `{ text: string, session_id?: string }` | `void` | Spawns a tokio task driving `run_agent_turn`; events flow back via `agent_event` listener. |
| `stop_turn` | `void` | `void` | Cancels the in-flight `CancellationToken`. |
| `tool_approval_response` | `{ id: number, decision: "allow" \| "deny" \| "auto_accept" }` | `void` | Resolves the oneshot. |
| `list_sessions` / `load_session` / `save_session` / `rename_session` / `delete_session` / `clear_sessions` | — | session metadata | Thin wrappers over `aictl_core::session::*`. |
| `new_incognito_session` | `void` | session id | |
| `list_agents` / `read_agent` / `save_agent` / `delete_agent` / `list_remote_agents` / `pull_agent` | — | agent metadata | Wrappers over `aictl_core::agents`. |
| `list_skills` / `read_skill` / `save_skill` / `delete_skill` / `list_remote_skills` / `pull_skill` | — | skill metadata | Wrappers over `aictl_core::skills`. |
| `config_get` / `config_set` / `config_unset` / `config_dump` | — | `String` / `void` | Wrappers over `aictl_core::config`. |
| `keys_set` / `keys_unset` / `keys_status` | — | — | Wrappers over `aictl_core::keys`. |
| `list_hooks` / `enable_hook` / `disable_hook` / `test_hook` | — | hook metadata | |
| `list_mcp_servers` / `enable_mcp_server` / `disable_mcp_server` / `restart_mcp` / `mcp_tool_catalogue` | — | server metadata | |
| `list_plugins` / `enable_plugin` / `disable_plugin` | — | plugin metadata | |
| `list_tools` | — | tool catalogue | Read-only. |
| `stats` | — | aggregate stats | |
| `balance_provider` / `list_balances` | — | per-provider balance/quota | |
| `list_models` / `set_model` / `set_provider` | — | model catalogue + active selection | |
| `gguf_browse` / `gguf_pull` / `mlx_browse` / `mlx_pull` | — | model lists, progress | Feature-gated. |
| `version` / `check_for_update` | — | version + remote version | |
| `reveal_audit_log` / `reveal_config_dir` | — | `void` | Spawns Finder via `tauri-plugin-shell`. |
| `compact_session` / `clear_chat` / `retry_last` / `undo_last` | `{ session_id }` | — | |

### 7.2 Events (Rust → webview)

A single channel name, `agent_event`, carrying the `AgentEvent` enum from §5.1 plus the desktop-specific `ToolApprovalRequest`. Other emitter channels:

- `session_changed` — fired when a session list mutation occurs (rename, delete, new) so all open settings panels resync.
- `download_progress` — feature-gated GGUF/MLX downloads.
- `update_available` — periodic update check result.

---

## 8. Visual design & component plan

The desktop adopts the design language defined in `website/DESIGN.md`. The website's `style.css` is a usable starting point — copy the tokens (`:root` block) and component primitives, then extend.

### 8.1 Layout

```
┌─────────────────────────────────────────────────────────────┐
│  ⌘  aictl                                  [model]  [stop]  │  ← title bar (custom, hidden traffic-lights kept)
├──────────┬──────────────────────────────────────────────────┤
│          │                                                  │
│ Sessions │   Conversation (message list, virtualized)       │
│ Agents   │                                                  │
│ Skills   │                                                  │
│ Tools    │                                                  │
│ Stats    │                                                  │
│ Settings │                                                  │
│          │                                                  │
│          ├──────────────────────────────────────────────────┤
│          │   Composer (multiline, ⌘↩ to send, /commands)    │
└──────────┴──────────────────────────────────────────────────┘
```

- Sidebar: 240 px, collapsible to 56 px (icons only). Selected tab determines the right pane in **non-chat** modes; for chat the sidebar is just navigation and the main pane stays the conversation.
- Title bar is custom (`titleBarStyle: "Overlay"` / `"hiddenInset"`) so the toolbar can host model picker and stop button cleanly.

### 8.2 Components & tokens

- All tokens (`--bg`, `--fg`, `--accent`, `--border`, `--font-mono`, `--font-sans`, etc.) ported from `website/style.css`.
- Buttons: GeistMono uppercase, 1.4 px tracking, 0 px radius. Primary = white-on-dark; ghost = bordered.
- Cards / panels: `rgba(255,255,255,0.03)` surface, `1px solid rgba(255,255,255,0.1)` border, no shadow.
- Hover dims to `rgba(255,255,255,0.5)` (never brightens) — this is unusual but consistent with the rest of the brand.
- Focus ring: `rgb(59,130,246)/0.5` for keyboard accessibility. Required on every interactive element.
- Cyan accent (`#5ed3f3`) only on: blinking cursor in composer, active-section kicker, command-prompt glyphs in code samples, the Stop button when active.
- Code blocks in answers: `highlight.js` theme tuned to the palette — base background `rgba(255,255,255,0.03)`, comments at 50% opacity, no syntax-rainbow vomit.

### 8.3 Markdown rendering

- `markdown-it` with `highlight.js` for fences. Streamed answers re-parse on every `StreamChunk` event using a lightweight diff against the last render — Solid's reactivity makes the cost negligible. The CLI's `StreamState`/tool-XML guard runs in core, so the webview never sees `<tool>` markup.

### 8.4 Tool approval modal

- Sheet attached to the chat window (macOS native sheet behaviour via Tauri). Shows tool name (GeistMono uppercase tag), tool body (collapsible JSON/XML viewer), and three buttons: Allow / Deny / Always allow this tool.
- Keyboard: ↩ = Allow, Esc = Deny, ⌘A = Always allow.
- A "Why is this tool dangerous?" disclosure surfaces the security policy's reason if the call would have been denied.

---

## 9. Phased rollout

### Phase 0 — Scaffolding (1–2 days)

- Add `crates/aictl-desktop` to the workspace.
- Tauri v2 init, macOS bundle config, Vite + Solid scaffold.
- "Hello world" Tauri command — confirm round-trip from webview to Rust and back.
- CI: macOS-only build job.

**Exit criterion**: `cargo build --bin aictl-desktop` produces a runnable `.app`; the window opens.

### Phase 1 — Core API stabilization (2–3 days)

- Add `AgentEvent` enum to `aictl-core::ui::events`.
- Add `confirm_tool_async` default-implemented method on `AgentUI`.
- Add helper `aictl_core::run::run_agent_session` (collapsing duplicate call sites in CLI).
- Verify CLI tests still pass (`cargo test --workspace`).

**Exit criterion**: CLI behaviour unchanged; new types compile; `aictl-core::ui::events::AgentEvent` is `Serialize`.

### Phase 2 — Minimal chat (3–5 days)

- `DesktopUI` impl, `send_message` command, `agent_event` listener in webview.
- Composer + message list with markdown rendering; streamed text updates live.
- Stop button → `stop_turn`.
- Read provider/model from existing `~/.aictl/config` — no settings UI yet, but the desktop now uses the same keys the user already has.

**Exit criterion**: a user can install the `.app`, open it, and have a conversation that streams, with the same agent loop the CLI uses, against the same `~/.aictl/config`.

### Phase 3 — Tool approval & sessions (3–4 days)

- Tool approval modal wired to the async oneshot.
- Sessions sidebar: list, new, switch, rename, delete, clear-all, incognito.
- Toolbar: Stop, Clear, Compact, Retry, Undo.

**Exit criterion**: feature parity with the CLI's chat + sessions + tool approval.

### Phase 4 — Agents & skills (2–3 days)

- Agents tab: list (with origin badges), browse remote, pull, edit, delete.
- Skills tab: same; user-defined skills surface as `/<name>` autocompletions in the composer.

**Exit criterion**: Agents and skills installed via the desktop are visible to the CLI on the next invocation.

### Phase 5 — Settings (4–5 days)

- Settings window with tabs: General, Provider & Models, Keys, Security, Memory, Hooks, MCP, Plugins, Tools, Local Models (gguf/mlx feature-gated).
- Each tab is a thin form over `config_get`/`config_set` and the relevant subsystem CRUD.
- Keys use the keyring (`aictl_core::keys`) — no plain-text fallback in the GUI.

**Exit criterion**: every CLI slash command we marked as "carry over" in §3 has a matching GUI control. Configuration changes round-trip with the CLI.

### Phase 6 — Stats, balance, model picker, polish (2–3 days)

- Stats tab.
- Balance probe per provider.
- About window, update check via `tauri-plugin-updater`.
- Keyboard shortcut overlay (⌘/).
- Empty states and error toasts (warnings via `warn_global`).

**Exit criterion**: app feels complete; no command-palette fallback needed.

### Phase 7 — Packaging & release (2–3 days)

- DMG build via `tauri build --target aarch64-apple-darwin` and `x86_64-apple-darwin`; universal binary if Tauri's bundler supports it cleanly, otherwise two DMGs.
- Manual code signing & notarization for the first release. Document the steps in `crates/aictl-desktop/README.md`.
- Add a "Desktop" section to `website/index.html` with download buttons.
- Update top-level `README.md` with desktop install instructions.
- Remove the **Desktop** entry from `ROADMAP.md`.

**Exit criterion**: a user on macOS 13+ can download a notarized DMG, drag-and-drop install, and use the app.

**Total effort estimate:** ~18–28 working days for one engineer, including review and bug fixes between phases. No phase is blocked on external work.

---

## 10. Testing & QA

- **Unit tests**: command wrappers in `crates/aictl-desktop/src/commands/` test argument parsing and error mapping. Business logic is in `aictl-core` and already covered.
- **Integration tests**: a Tauri-side test harness that boots a hidden window, drives `send_message` against the existing `Provider::Mock`, and asserts the event sequence. (Same Mock provider the CLI already uses for `cargo test`.)
- **Frontend tests**: Vitest for component-level reactivity (markdown render, tool approval modal). No e2e suite in v1 — manual smoke test.
- **Manual macOS smoke test checklist** (in the crate's README):
  1. Fresh install, no `~/.aictl/` — first-run flow renders an onboarding state with a "Set up provider" button that opens Settings → Provider & Models.
  2. Existing CLI user — opens the desktop, sees the same model, same sessions, same agents.
  3. Session created in desktop is loadable from CLI and vice versa.
  4. Tool approval round-trip with `read_file`, `exec_shell`, and an `mcp__*` tool.
  5. Streaming with a slow provider feels smooth (no jank, no torn markdown).
  6. Stop button cancels mid-stream.
  7. Quitting mid-turn cleans up MCP child processes (the existing `mcp::shutdown` is wired into the Tauri `RunEvent::Exit` handler).

---

## 11. Risks & open questions

| Risk | Mitigation |
|---|---|
| `WKWebView` quirks vs Chromium-based dev browser | Test in Tauri dev mode early; avoid features that don't ship in WebKit (e.g., recent CSS only-Chrome props). |
| Async tool approval changes the trait shape | Default impl preserves CLI behaviour; migration is one-line on the call site. |
| Code signing / notarization friction | Manual for v1; document the exact `xcrun notarytool` steps; automate in v2. |
| Universal binary builds | If `tauri-bundler` chokes, ship two DMGs (arm64 + x86_64). Functional parity is what matters. |
| `gguf` / `mlx` feature flags differ between CLI and Desktop builds | Default desktop release builds without these — local inference is a CLI power-user feature. Advanced users can build from source with `--features gguf,mlx`. |
| Keychain prompts on every key read | The keyring crate caches; if the per-prompt friction is bad, gate behind one "Allow always" prompt at install time. |
| Hooks that read stdin and spawn subprocesses | Already shell-isolated by `aictl_core::hooks` — desktop inherits the existing security boundary verbatim. |

### Open questions to resolve before Phase 2

1. **Solid vs Svelte** — both fit. Solid has a slightly smaller runtime and better TS ergonomics; Svelte 5 has Runes but more churn. Default to Solid; revisit if the team has a strong preference.
2. **Single window vs multi-window for sessions** — defer to v2.
3. **Native menu bar items** — at minimum: File (New session, New incognito, Close), Edit (Cut/Copy/Paste/Find), View (Reload, toggle sidebar), Window, Help. Standard macOS menu via Tauri's menu API; not on the critical path.
4. **Auto-update channel** — single "stable" channel via Tauri updater; signing key generation is part of Phase 7.

---

## 12. Out-of-scope follow-ups (post v1)

- Linux (`.AppImage` / `.deb`) and Windows (`.msi`) builds.
- A separate menu-bar app for one-shot prompts (the "raycast" experience).
- Multi-window sessions.
- Built-in diff viewer for the future coding-agent workflow.
- Reusing the webview bundle for an `aictl-server`-hosted web UI.
