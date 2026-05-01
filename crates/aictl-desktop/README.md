# aictl-desktop

Native macOS frontend for [`aictl`](../../). Built on Tauri v2 with a
Solid + Vite webview, sharing every config file (`~/.aictl/config`,
sessions, agents, skills, MCP, hooks, plugins, audit log, stats) with
the CLI. Design rationale and roadmap live in
[`.claude/plans/desktop-app.md`](../../.claude/plans/desktop-app.md).

## Status

Foundational scaffold landed. What works today:

- Workspace member registration (`crates/aictl-desktop`).
- macOS-only `cfg` gate on the binary; non-macOS builds exit with a
  clear message.
- `Role::Desktop` and `AICTL_WORKING_DIR_DESKTOP` plumbed through the
  engine, with a no-workspace sentinel in `security::load_policy`.
- `DesktopUI` implementation of `aictl_core::AgentUI`, emitting
  `AgentEvent` over Tauri's `agent_event` channel.
- Tauri commands for chat (`send_message`, `stop_turn`,
  `tool_approval_response`), workspace lifecycle (`get_workspace`,
  `set_workspace`, `pick_workspace`), sessions (list/load/delete/
  incognito), and a couple of system entries.
- Solid + Vite frontend with a chat surface, composer, tool-approval
  modal, and workspace onboarding card.

What's not done (later phases of the plan):

- Session persistence end-to-end (history hydration / save on every
  turn).
- Settings panes (Provider, Keys, Security, Memory, Hooks, MCP,
  Plugins, Tools, Local Models).
- Stats and balance probe surfaces.
- Agents / skills CRUD UI (engine has the APIs; UI is stubbed).
- Code-signing, notarization, DMG bundling.

## Building

The desktop crate is **excluded from the workspace's default member
set** so a bare `cargo build` / `cargo lint` / `cargo test` keeps
working on every platform. Build it explicitly on macOS:

```bash
# Rust side only — useful for type-checking.
cargo build -p aictl-desktop

# Full Tauri dev workflow (requires Node.js).
cd crates/aictl-desktop/webview
npm install
cd ..
cargo tauri dev    # if cargo-tauri is installed
# OR
cargo run --bin aictl-desktop    # after `npm run build` populated webview/dist
```

The `frontendDist` referenced from `tauri.conf.json` is
`crates/aictl-desktop/webview/dist`. A placeholder `index.html` ships
in the repo so `cargo build` succeeds before the npm bundle exists.

## Workspace folder

The desktop runs every tool call inside a folder the user picks at
first launch (Settings → Workspace later). The path is stored in
`~/.aictl/config` as `AICTL_WORKING_DIR_DESKTOP` and is **independent
of the CLI's `AICTL_WORKING_DIR`** — pinning one binary doesn't
silently change the other. Until a workspace is set, the security
policy refuses every CWD-relative tool call with
`"no workspace selected"`. See plan §5.4.

## Layout

```
crates/aictl-desktop/
├── Cargo.toml                # depends on aictl-core; macOS-aware
├── tauri.conf.json           # Tauri v2 app config
├── build.rs                  # tauri_build::build()
├── capabilities/default.json # webview permissions (dialog, clipboard, …)
├── icons/icon.png            # placeholder; replace before release
├── src/
│   ├── main.rs               # macOS bin guard
│   ├── lib.rs                # Tauri builder + plugin wiring
│   ├── ui.rs                 # DesktopUI: AgentUI → AgentEvent stream
│   ├── chat.rs               # Drives run_agent_turn for one message
│   ├── workspace.rs          # AICTL_WORKING_DIR_DESKTOP helpers
│   ├── state.rs              # Shared mutable state (turn cancel, pending approvals)
│   └── commands/             # #[tauri::command] handlers
└── webview/                  # Solid + Vite frontend
    ├── package.json
    ├── vite.config.ts
    ├── tsconfig.json
    ├── index.html
    └── src/
        ├── App.tsx
        ├── main.tsx
        ├── lib/{ipc.ts,markdown.ts}
        ├── components/{Chat,Composer,ToolApproval,Sidebar,Titlebar,EmptyWorkspace}.tsx
        └── styles/{tokens.css,components.css}
```
