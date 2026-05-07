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
- DMG bundling polish (background, icon position).

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

## Releasing (signed + notarized)

`scripts/release-mac.sh` builds, signs with the Developer ID
certificate, submits to Apple for notarization, and staples the ticket
in one shot. It reads credentials from `~/.aictl/release.env` (preferred)
or the current shell environment:

```bash
# ~/.aictl/release.env
APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
APPLE_ID="you@example.com"
APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx"   # app-specific password
APPLE_TEAM_ID="TEAMID"
```

Then:

```bash
chmod 600 ~/.aictl/release.env
crates/aictl-desktop/scripts/release-mac.sh
```

Artifacts land in `target/release/bundle/{macos,dmg}/`. The script runs
`codesign --verify` and `spctl -a -t exec` afterwards as a sanity check.

The bundle identifier (`com.piotrwittchen.aictl`) and entitlements
(`entitlements.plist` — hardened runtime + JIT/dyld relaxations the
WKWebView needs) are committed; only credentials live outside the repo.

### Releasing via GitHub Actions

The `RELEASE` workflow (`.github/workflows/release.yml`) signs and
notarizes both the `.app` and the `.dmg` automatically when a `v*`
tag is pushed. The `build-desktop` job replicates the same five-step
flow as the local script: import cert → sign `.app` → notarize+staple
`.app` → sign DMG → notarize+staple DMG. A scratch keychain is
created at the start and torn down in an `if: always()` cleanup step
so credentials never persist on the runner.

Required repository secrets (Settings → Secrets and variables →
Actions):

| Secret | Value |
|---|---|
| `MACOS_CERTIFICATE` | `base64 -i devid.p12` of the exported cert |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `KEYCHAIN_PASSWORD` | any random string (e.g. `openssl rand -hex 16`) — only used for the temp keychain |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | Apple ID email |
| `APPLE_PASSWORD` | app-specific password from appleid.apple.com |
| `APPLE_TEAM_ID` | 10-char team identifier |

Generate the base64 cert blob locally:

```bash
base64 -i ~/Desktop/devid.p12 | pbcopy   # paste into MACOS_CERTIFICATE
```

If any secret is missing, the desktop build will fail loudly rather
than silently producing an unsigned bundle.

### Updater signing key

The in-app update flow (download → install → restart) is driven by
[`tauri-plugin-updater`](https://v2.tauri.app/plugin/updater/), which
verifies a minisign signature over the downloaded `.app.tar.gz`
**independently** of Apple's Developer ID codesign. The keypair is
yours alone — generate it once, commit the public half, keep the
private half offline.

Generate the pair locally:

```bash
npm install -g @tauri-apps/cli@^2          # if not already installed
tauri signer generate -w ~/.aictl/updater.key
```

The command prints two values:

- the **public key** (base64 string, ~80 chars) — paste it into the
  `plugins.updater.pubkey` field of
  [`tauri.conf.json`](tauri.conf.json), replacing the
  `REPLACE_WITH_TAURI_UPDATER_PUBKEY` placeholder.
- the **private key file** at `~/.aictl/updater.key` — never commit
  this. Treat it like the Developer ID `.p12`.

Add two more repo secrets so the release workflow can sign the
updater archive:

| Secret | Value |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | contents of `~/.aictl/updater.key` (paste the whole multi-line file) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the password you entered when generating the key |

The next tag push will produce, for each macOS arch:

- `aictl-desktop-darwin-<arch>.dmg` (existing — for fresh installs)
- `aictl-desktop-darwin-<arch>.app.tar.gz` (new — for in-app updates)
- `aictl-desktop-darwin-<arch>.app.tar.gz.sig`
- `latest.json` (the manifest the running app fetches)

The app's update endpoint is
`https://github.com/<owner>/<repo>/releases/latest/download/latest.json`,
which auto-redirects to the most recent tag — there's no separate
hosting to maintain.

If a build runs without the secrets set, the
`Package updater archive` step fails with a clear error rather than
silently producing an unsigned tarball that the running app would
refuse to install.

## Workspace folder

The desktop runs every tool call inside a folder the user picks at
first launch (Settings → Workspace later). The path is stored in
`~/.aictl/config` as `AICTL_WORKING_DIR_DESKTOP` and is **independent
of the CLI's `AICTL_WORKING_DIR_CLI` (or legacy `AICTL_WORKING_DIR`)** — pinning one binary doesn't
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
