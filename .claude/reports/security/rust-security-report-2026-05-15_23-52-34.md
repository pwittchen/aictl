# Security Report — 2026-05-15 23:52:34

## Dependency Audit
- `cargo audit`: **0 vulnerabilities**, 19 unmaintained / unsound warnings across **789 crate deps** (advisory db 1090 entries). All 19 are transitive through `tauri` / `wry` / GTK3 bindings — the desktop crate only. CLI and server are unaffected. **LOW**
- `glib 0.18.5` `RUSTSEC-2024-0429`: unsound `Iterator` impl for `VariantStrIter`. Reaches the desktop only via WebKit; no aictl code constructs a `VariantStrIter`. **LOW**
- `Cargo.lock` committed; no wildcard versions in any `Cargo.toml`. `resolver = "3"`. **INFO (strength)**
- `keyring` v3 wired with `apple-native` / `windows-native` / `sync-secret-service` features — no silent mock-store fallback. **INFO (strength)**
- 789-crate transitive set is dominated by `tauri` / `axum` / `reqwest`-rustls / `aws-lc-sys`; CLI-only build is much smaller. **INFO**

## Unsafe Code
- **Zero `unsafe` blocks / functions** anywhere in `crates/*/src/`. Only `unsafe` reference is the string `"entry has unsafe path"` in `crates/aictl-core/src/tools/archive.rs:431` (zip-slip prevention message). **INFO (strength)**

## Command Execution
- `tools/shell.rs:4` `tool_exec_shell` still runs LLM-supplied strings via `sh -c`. Gated by `security::validate_tool` → `check_shell` (blocklist, subshell block) + `env_clear` + `scrubbed_env` + `kill_on_drop` + `shell_timeout`. `--unrestricted` disables every gate but does not bypass hooks. **MEDIUM (inherent, well-mitigated)**
- `commands/update.rs:73,123,176` and `aictl-server/src/update.rs:73` still run `sh -c "curl -fsSL …/install.sh | sh"` with **no checksum/signature verification**. HTTPS is enforced via the URL (`aictl.app`), but a compromised origin or stolen cert would execute arbitrary code. **MEDIUM**
- `tools/run_code.rs:117` interprets LLM-supplied source via Python/Node/etc. Not validated by `security::validate_tool` (`security.rs:443` default arm). Inherits CWD jail, env scrub, `kill_on_drop`, timeout — but the interpreter itself can read/write the workspace and reach the network. Equivalent risk to `exec_shell` but **not in the validation switch**. **MEDIUM**
- `hooks.rs:444` runs user-configured `sh -c <cmd>`. Cleanly scoped (user-supplied lifecycle, `env_clear`, `kill_on_drop`, timeout). **INFO**
- `plugins.rs:553` spawns plugin entrypoint directly (no shell), `env_clear` + `scrubbed_env` + `kill_on_drop`. Gated behind `AICTL_PLUGINS_ENABLED=true`. **INFO (strength)**
- `mcp/stdio.rs:53` spawns MCP servers with `env_clear` + `scrubbed_env` + `kill_on_drop`; gated behind `AICTL_MCP_ENABLED=true`. **INFO (strength)**
- `tools/git.rs`, `tools/clipboard.rs`, `tools/system_info.rs`, `tools/list_processes.rs`, `tools/json_query.rs`, `tools/lint.rs` invoke fixed binaries with structured args (no `sh -c` interpolation). **INFO**

## Input Validation
- `--agent <name>` now validated via `agents::is_valid_name` at `crates/aictl-cli/src/main.rs:983` before `read_agent` — prior local-file-exfil path closed. **INFO (fixed since last audit)**
- `agents.rs:71` / `skills.rs::is_valid_name`: `[A-Za-z0-9_-]+`, non-empty — prevents `..`, slashes, null bytes. **INFO (strength)**
- `session::normalize_name` restricts session names to `[a-z0-9_]`. **INFO**
- `security::detect_prompt_injection` (`security.rs:1239`) covers ignore-previous / override / jailbreak corpus; runs at REPL boundary. **INFO (strength)**
- clap derive for CLI args; rustyline for REPL — no unbounded raw reads. **INFO**
- Tool XML parser is substring-based, no regex backtracking risk. **INFO**

## Network Security
- All provider endpoints are `https://` (`llm/*.rs`). Server passthrough hard-codes `https://api.anthropic.com/v1/messages`. **INFO (strength)**
- **SSRF still wide open**: `tools/web.rs:309` `fetch_url` and `tools/web.rs:359` `extract_website` accept arbitrary URLs from the LLM. **No scheme allowlist, no private-IP / link-local / loopback / cloud-metadata block** (`169.254.169.254`, `metadata.google.internal`, `localhost`, `10.*`, `172.16-31.*`, `192.168.*`), **no `file://` block**, **no redirect cap**, **no body-size cap**. `security::validate_tool` explicitly skips both (`security.rs:443` default arm). **HIGH (carried from prior audit)**
- `tools/geo.rs:4,7` `fetch_geolocation` still hits **`http://ip-api.com`** in cleartext; IP/location response is observable and tamperable on-path. **MEDIUM (carried)**
- **No default `reqwest::Client` timeout**: `config.rs:48` `http_client()` still uses `reqwest::Client::new()`. Every LLM call (`llm/*.rs`), `fetch_url`, `extract_website`, `search_web_*`, `fetch_geolocation` can hang indefinitely. LLM calls are wrapped in `tokio::time::timeout(llm_timeout())` (`run.rs`), but tool calls have no host-side cap. **MEDIUM (carried)**
- MCP outbound URLs validated via `security::validate_mcp_url` at config-parse time **and** re-validated on every dispatch (`mcp/http.rs:78`, `mcp/sse.rs:176`) — scheme + host allow/deny + HTTPS-by-default. **INFO (strength)**
- `aictl-server`: master-key auth (constant-time compare, `auth.rs:21`), per-IP token bucket rate limit (`auth.rs:91`), request timeout layer + body limit layer (`main.rs:307-312`), non-loopback bind warning (`main.rs:205-214`). CORS off by default; no wildcard origin support. **INFO (strength)**
- Server has no built-in TLS — bind defaults to loopback; deploying on a non-loopback bind requires a TLS terminator (documented). **LOW**

## Secrets Management
- `keys::get_secret` resolves overrides → keyring → plain config; `set_secret` removes plain shadow when writing keyring (`keys.rs:213`). Clean migration via `lock_key` / `unlock_key`. **INFO (strength)**
- `AICTL_SERVER_MASTER_KEY` auto-generated via `getrandom::fill` (32 bytes, base64url no-pad) and persisted via the same `keys::set_secret` pipeline (`master_key.rs:75-82`). Printed once to stderr; auth uses constant-time compare. **INFO (strength)**
- `config::config_set` (`config.rs:1020`) still writes `~/.aictl/config` with `std::fs::write` and **no explicit `chmod 0600`** — when the keyring is unavailable or after `/unlock-keys`, plain-text API keys (including `AICTL_SERVER_MASTER_KEY` and `AICTL_CLIENT_MASTER_KEY`) land at whatever the umask permits, often `0644`. Same gap as last audit. **HIGH (carried)**
- `~/.aictl/audit/<session-id>` (audit log), `~/.aictl/sessions/<id>.json` (transcripts), `~/.aictl/memory.json`, `~/.aictl/stats` — all written via `fs::write` / `OpenOptions::append` with no mode-set. Audit logs and session transcripts can contain user prompts, tool outputs, and (if not redacted) any secret material the LLM saw. **MEDIUM**
- Provider error bodies are passed through to user-visible errors (`error.rs::from_http`, `llm/anthropic.rs:226`). If a provider ever echoes `x-api-key` / `Authorization` in an error envelope, it surfaces in REPL output and gets persisted to the session. No scrub on the inbound error path. **LOW (carried)**
- `redact_outbound` (`run.rs`) does NOT run on inbound error bodies — only on outbound message content. **LOW**
- **Redaction defaults to `Off`** (`redaction.rs:429`). Out of the box, API keys / JWTs / passwords typed into the REPL are sent verbatim to the LLM provider. Documented, but worth flagging — the safer default would be `redact`. **MEDIUM**

## File System Security
- CWD jail (`security::check_path_with`) canonicalizes existing paths, manually normalizes `..` for non-existent paths, rejects null bytes, blocked-path list covers `~/.ssh`, `~/.gnupg`, `~/.aictl`, `~/.aws`, `~/.config/gcloud`, `/etc/shadow`, `/etc/sudoers`. **INFO (strength)**
- Workspace carve-out: `~/.aictl/workspace/` is usable while `~/.aictl/keys`, `~/.aictl/audit`, `~/.aictl/config` remain blocked. **INFO (strength)**
- **TOCTOU window** between `check_path_write` and `tokio::fs::write` / `tokio::fs::create_dir_all` (`tools/filesystem.rs:143,163,900`). A local attacker who can race to plant a symlink at the resolved target between validation and the write redirects the write outside the jail. Local-only; needs write access to CWD or its parent. **LOW (carried)**
- `write_file` / `edit_file` use `tokio::fs::write` (truncate-and-replace), not `OpenOptions::create_new(true)` or atomic `rename` from a temp file — TOCTOU mitigation absent. **LOW**
- `list_directory` labels symlinks (`tools/filesystem.rs:179`); read/write tools follow symlinks transparently. **LOW**
- `archive` tool: `tools/archive.rs:431` rejects zip entries with `..` / absolute paths — zip-slip mitigated. **INFO (strength)**
- `write_file` size cap honored via `max_file_write_bytes` (`security.rs:387`, default 1 MB). **INFO**

## Error Handling & Info Leaks
- `AictlError::Auth { provider, status, body }` includes the raw response body in its `Display` (`error.rs:25`). If a provider echoes auth headers in a 401 body, they leak into REPL output and the persisted transcript. No client-side scrub on the inbound error path. **LOW (carried)**
- `unwrap()` / `expect()` outside tests: a handful remain in non-fatal paths (`llm/gguf.rs:411` post-init backend access, `llm/balance.rs:296`-`297` in test, `master_key.rs:80` OS RNG — only triggers on a misconfigured sandbox). No new panic gateways added. **LOW**
- Release-mode errors do not expose stack traces or internal file paths beyond user-supplied ones. **INFO**
- `aictl-server` returns 401 with **identical empty body** for missing header / wrong scheme / wrong key (`auth.rs:39-69`) — no token enumeration via response differences. **INFO (strength)**

## Denial of Service
- LLM responses bounded by `MAX_RESPONSE_TOKENS = 4096`; agent loop caps at 20 iterations per turn. **INFO (strength)**
- Tool output truncated to `MAX_TOOL_OUTPUT_LEN` with UTF-8 boundary walk-back. **INFO**
- **`fetch_url` / `extract_website` read the full response body** (`tools/web.rs:317,367`) before truncation — a hostile or accidentally huge page can OOM the process. **MEDIUM (carried)**
- **No default HTTP timeout** for tool fetches; only LLM calls are wrapped in `tokio::time::timeout(llm_timeout())`. **MEDIUM (carried)**
- Shell / interpreter timeout (default 30s) enforced via `tokio::time::timeout` + `kill_on_drop`. **INFO**
- MCP startup timeout (`AICTL_MCP_STARTUP_TIMEOUT`, default 10s) prevents a hung server from blocking init; per-RPC timeout from config. **INFO (strength)**
- `search_files_blocking` / `find_files_blocking` prefer `rg` (respects `.gitignore`); fallback walks `**/*` and is bounded by `MAX_TOOL_OUTPUT_LEN`. **LOW**
- `aictl-server`: per-IP token bucket rate limit, body-size limit, request timeout, graceful shutdown. **INFO (strength)**

## Summary

Overall posture: **7.5 / 10**. The codebase has matured substantially since the prior audit — new attack surfaces (`aictl-server`, MCP, plugins, hooks, coding-agent mode) all ship with the same disciplined posture: env scrubbing, `kill_on_drop`, scoped allow/deny lists, defense-in-depth re-validation. Still zero `unsafe`, no real vulns from `cargo audit` (only unmaintained transitives via Tauri/GTK3). The structural gaps from the last review remain the headline issues, plus one new finding (`run_code` outside the validation switch) and the reminder that **redaction defaults to off**.

**Critical (must fix)** — none.

**High (should fix)**
- **SSRF in `fetch_url` / `extract_website`** (`crates/aictl-core/src/tools/web.rs:309,359`): block private/loopback/link-local IPs, cloud metadata endpoints, `file://`, restrict to `http`/`https`, cap redirects, cap body size. The default arm at `crates/aictl-core/src/security.rs:443` lets these through.
- **`config::config_set` writes without `chmod 0600`** (`crates/aictl-core/src/config.rs:1020`): explicit `fs::set_permissions(&config_path, Permissions::from_mode(0o600))` after every write so plain-text API keys and master keys land user-only. Apply the same to `~/.aictl/audit/`, `~/.aictl/sessions/`, `~/.aictl/memory.json` (`crates/aictl-core/src/audit.rs:156,223`, `crates/aictl-core/src/session.rs:209`, `crates/aictl-core/src/memory.rs:116`).

**Medium (should fix)**
- Add `run_code` to `security::validate_tool` so the disabled-tools list works and audit/security gating is uniform with `exec_shell` (`crates/aictl-core/src/security.rs:380-443`).
- Set a default `reqwest::Client` timeout in `config::http_client` (`crates/aictl-core/src/config.rs:46-48`) — e.g., 120s connect/total — so tool fetches and the catalogue/healthz probe can't hang.
- Cap response body in `fetch_url` / `extract_website` (`tools/web.rs:317,367`) — stream with a `MAX_FETCH_BYTES` ceiling instead of `.text().await`.
- Switch `fetch_geolocation` off cleartext `http://ip-api.com` (`tools/geo.rs:4,7`).
- Harden the updater: ship signed checksums or sha256 for the install scripts (`commands/update.rs:5,9`, `aictl-server/src/update.rs:14`).
- Consider flipping the default redaction mode from `Off` to `Redact` (`security/redaction.rs:429`) — current default sends API keys / JWTs / passwords typed at the REPL straight to the provider.

**Low (nice to have)**
- Scrub `Authorization` / `x-api-key` from provider error response bodies before surfacing in `AictlError::{Auth,Provider}` (`crates/aictl-core/src/error.rs:25-37`, all `llm/*.rs` call sites).
- Use `OpenOptions::new().create_new(true)` or an atomic temp+rename in `write_file` / `edit_file` to shrink the TOCTOU window (`tools/filesystem.rs:143,900`).
- Pin redirect policy on the shared `reqwest::Client` (`reqwest::redirect::Policy::limited(5)`) so a 302 → `http://169.254.169.254/...` is bounded even before SSRF guards land.
