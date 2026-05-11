# aictl-desktop

Native macOS frontend for [`aictl`](../../). Built on Tauri v2 with a
Solid + Vite webview, sharing every config file (`~/.aictl/config`,
sessions, agents, skills, MCP, hooks, plugins, audit log, stats) with
the CLI. Design rationale and roadmap live in
[`.claude/plans/desktop-app.md`](../../.claude/plans/desktop-app.md).

## Status

Daily-driver shape. What works today:

- macOS-only `cfg` gate on the binary; non-macOS builds exit with a
  clear message.
- `Role::Desktop` and `AICTL_WORKING_DIR_DESKTOP` plumbed through the
  engine, with a no-workspace sentinel in `security::load_policy`.
- `DesktopUI` implementation of `aictl_core::AgentUI`, emitting
  `AgentEvent` over Tauri's `agent_event` channel.
- Sessions sidebar, chat surface, composer, tool-approval modal,
  workspace onboarding, and a horizontally-resizable three-pane layout
  (sidebar / chat / files) with per-pane widths persisted across
  launches.
- Workspace files pane: tree view of the active workspace with create,
  rename, upload-from-disk, and modal-confirmed delete actions on
  files and directories. The pane's visibility and last-open file are
  remembered between launches; if the open file disappeared while the
  app was closed, the pane forces itself open so the user lands on
  something useful.
- Editor pane with syntax highlighting via highlight.js (language
  picked from the file extension, with `Dockerfile` / `Makefile`
  bare-name fallbacks; unknown extensions render as plain text).
- Settings window with Provider, Keys, Local Models, Security,
  Memory, Hooks, MCP, Plugins, Tools, and Agents/Skills CRUD panes.
- In-app updater with minisign verification (download → install →
  restart) on top of Apple's Developer ID codesign.
- Auto-open update dialog on startup when a newer release is
  available.
- Stats and balance probe surfaces share the same engine paths as the
  CLI.

### Desktop-only config keys

Persisted in `~/.aictl/config` alongside the shared `AICTL_*` keys.
The Tauri Settings IPC gate whitelists exactly this set:

| Key | Purpose |
|---|---|
| `AICTL_WORKING_DIR_DESKTOP` | Workspace folder (CWD jail root) for the desktop. Independent of `AICTL_WORKING_DIR_CLI`. |
| `AICTL_DESKTOP_DENSITY` | UI density (`comfortable` / `compact`). |
| `AICTL_DESKTOP_NOTIFICATIONS` | Toggle macOS notifications on long-running tool completion. |
| `AICTL_DESKTOP_SIDEBAR_VISIBLE` | Sessions sidebar shown / hidden across launches. |
| `AICTL_DESKTOP_FILES_VISIBLE` | Workspace files pane shown / hidden across launches. |
| `AICTL_DESKTOP_OPEN_FILE` | Last file opened in the editor pane; rehydrated on launch (path is verified — dead paths are forgotten). |
| `AICTL_DESKTOP_SIDEBAR_WIDTH` | Pixel width of the sessions sidebar; bounded on hydration so a hand-edited value can't collapse the pane. |
| `AICTL_DESKTOP_EDITOR_WIDTH` | Pixel width of the chat / editor column. |
| `AICTL_DESKTOP_FILES_WIDTH` | Pixel width of the files pane. |

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

The same release workflow now also signs and notarizes the CLI
(`aictl`) and `aictl-server` binaries when they're built on macOS
targets — the `build` job reuses the scratch-keychain pattern, signs
each binary with the per-crate `entitlements.plist`, and submits a zip
to `notarytool`. Bare CLI binaries cannot be stapled, so Gatekeeper
resolves the notarization ticket online on first launch.

### Shared Keychain across CLI / server / desktop

Without extra work, each signed binary would land in its own macOS
Keychain ACL — the first time the CLI tried to read an entry the
desktop had created (or vice versa), the user would be prompted for
their login password, and again for every other entry. The fix is a
`keychain-access-groups` entitlement shared by all three binaries,
which puts every aictl item into one shared partition.

The entitlement lives at
`crates/aictl-{cli,server,desktop}/entitlements.plist` with a
`__TEAM_ID__.com.piotrwittchen.aictl` placeholder. CI substitutes
`__TEAM_ID__` with the `APPLE_TEAM_ID` repo secret just before
`codesign`, so the team identifier stays out of source.

At runtime, `aictl-core` reads `AICTL_APPLE_TEAM_ID` at compile time
(via `option_env!`, tracked by `crates/aictl-core/build.rs`) and bakes
it into the access-group string used by every Keychain call. Release
CI exports `AICTL_APPLE_TEAM_ID` from the same `APPLE_TEAM_ID` secret
before each `cargo build`, so the entitlement on the binary and the
attribute on the Keychain item agree.

**Source builds are unaffected.** Contributors running `cargo run` from
a clone, or users who install via `cargo install` or a Homebrew
formula that compiles from source, get an ad-hoc-signed binary
without the entitlement and with no team ID baked in. The macOS
backend transparently falls back to the unscoped `keyring::Entry`
path, which itself falls back to plain `~/.aictl/config` — so building
from source keeps working, just without the shared-Keychain
experience. Per-binary Keychain prompts may still appear in that
configuration; the cure is installing the signed binaries from the
GitHub release page.

The welcome banner shows `keys: keychain (shared)` when the shared
path is live, `keys: keychain` when the binary fell back to the
unscoped path, and `keys: plain text` when no keyring backend is
available at all.

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
        ├── App.tsx                    # pane layout, drag handles, persisted widths
        ├── main.tsx
        ├── lib/{ipc.ts,markdown.ts,highlight.ts}
        ├── components/{Chat,Composer,ToolApproval,Sidebar,Titlebar,EmptyWorkspace,
        │               FilePane,EditorPane,ConfirmDelete,Settings,Toolbar,
        │               UpdateModal,ContextDetails,AgentEditor,SkillEditor,
        │               McpEditor,CreatePrompt,ProviderSetup}.tsx
        └── styles/{tokens.css,components.css}
```
