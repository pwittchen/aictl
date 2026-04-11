# Security Report — 2026-04-11 23:21:59

## Dependency Audit
- `cargo audit`: clean — 0 vulnerabilities across 300 crate dependencies (advisory db 1043 entries). **INFO**
- `Cargo.lock` committed to VCS; no wildcard (`*`) version requirements in `Cargo.toml`. **INFO**
- `keyring` v3 built with explicit `apple-native`, `windows-native`, `sync-secret-service` features — avoids silent mock-store fallback. **INFO (strength)**
- Supply chain exposure: 300 transitive deps for a single-binary CLI is moderate; `scraper`/`html5ever` and `aws-lc-sys` (native C) are the largest attack surface. **LOW**

## Unsafe Code
- **No `unsafe` blocks or functions** anywhere in `src/`. **INFO (strength)**

## Command Execution
- `tools.rs:105` `tool_exec_shell` runs LLM-supplied strings via `sh -c`. Mitigated by `security::validate_tool` (blocklist, subshell block, env scrub, timeout). Inherently risky but gated; `--unrestricted` disables all gates. **MEDIUM**
- `commands.rs:1318`, `commands.rs:2169` run `sh -c "curl -sSf …/install.sh | sh"` with no signature/checksum verification (`UPDATE_CMD` at `commands.rs:1285`). A compromised raw.githubusercontent.com or TLS MITM would execute arbitrary code. **MEDIUM**
- `tools.rs:530` `tool_fetch_datetime` spawns `date` with fixed args (no injection path). **LOW**
- `commands.rs:158` `copy_to_clipboard` spawns `pbcopy` with no args; safe. **INFO**
- No user-controlled input is interpolated into a `Command::new` argument list; all dynamic exec goes through `sh -c` with the gating above. **INFO**

## Input Validation
- `main.rs:1211` **CLI `--agent <name>` is not validated with `agents::is_valid_name`** before `agents::read_agent` joins it into `~/.aictl/agents/<name>` and passes to `fs::read_to_string`. A value like `../../../etc/passwd` reads an arbitrary local file; its contents are then appended to the system prompt and **exfiltrated to the LLM provider**. The interactive code paths (`commands.rs:2269/2372`) do validate. **MEDIUM**
- REPL `run_agent_turn` gates user input through `detect_prompt_injection` — high-quality keyword + tag list covers the common override/jailbreak corpus (`security.rs:750-838`). **INFO (strength)**
- `session::normalize_name` restricts session names to `[a-z0-9_]`; tab-separated `.names` format is safe from field injection. **INFO**
- Agent file list filters via `is_valid_name`, so stray files can't be loaded by the menu (`agents.rs:85`). **INFO**
- clap derive provides bounded arg parsing; REPL line input uses rustyline (no unbounded raw reads). **INFO**
- Tool XML parser is a substring matcher with no regex/allocation explosion risk. **INFO**

## Network Security
- All provider endpoints use `https://` (`llm_*.rs`, `tools.rs:323`). **INFO**
- `tools.rs:550` `fetch_geolocation` uses **`http://ip-api.com`** in cleartext — the user's IP/location response can be observed or tampered with on-path. ip-api.com requires a paid plan for HTTPS; switching to `https://ipapi.co/json` or similar would fix it. **MEDIUM**
- **No `reqwest::Client` timeout** set in `config::http_client()` (`config.rs:9`). Provider LLM calls (`llm_openai/anthropic/gemini/grok/mistral/deepseek/kimi/zai`), `search_web`, `fetch_url`, `extract_website`, `fetch_geolocation` can hang indefinitely. Only `fetch_remote_version` (3s), `run_issues` (10s), and Ollama list (2s) set per-request timeouts. Hanging LLM calls are user-cancelable via Esc in the REPL but not in `--auto --quiet`/`-m` single-shot mode. **MEDIUM**
- **SSRF: `fetch_url` / `extract_website` accept arbitrary URLs** from the LLM with no scheme allowlist, no private-IP / link-local / metadata-endpoint blocking, and no max body size (`tools.rs:419-527`). An LLM (or prompt-injected web result the agent is reading) can drive the tool to fetch `http://169.254.169.254/…` (cloud metadata), `http://10.*`, `http://localhost:…`, or `file:` via redirect. `security::validate_tool` explicitly passes these through (`security.rs:328`). **HIGH**
- `resp.text().await` loads the full response body into memory before any truncation — a multi-GB page can OOM the process. **MEDIUM**

## Secrets Management
- `keys::get_secret` prefers OS keyring (Keychain / Credential Manager / Secret Service) with plain-text fallback. Clean separation with `KeyLocation::{None,Config,Keyring,Both}` migration primitives. **INFO (strength)**
- Verified via grep: API keys are **never printed, logged, or included in error messages**. Header insertion uses `format!("Bearer {api_key}")` directly into the request, not any log sink. **INFO**
- `config::config_set` (`config.rs:192`) writes `~/.aictl/config` via plain `std::fs::write`, honoring the process umask — **no explicit `0600` permissions**. On a shared system where umask allows group/world-read, plain-text API keys (and keys migrated back via `/unlock-keys`) will land with `0644`. **HIGH**
- Subprocess env scrubbing (`security.rs:689-727`) strips `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD`, and anything containing SECRET/PASSWORD/CREDENTIAL before `exec_shell`. **INFO (strength)**
- `--clear-keys` CLI flag calls `run_clear_keys_unconfirmed` (`main.rs:1144`) — destructive operation with **no confirmation prompt** on the CLI path, unlike REPL `/clear-keys`. **LOW**

## File System Security
- CWD jail (`security.rs:574-668`) canonicalizes existing paths and manually normalizes `..` for non-existent paths — defeats path traversal for file tools. **INFO (strength)**
- Blocked-path list covers `~/.ssh`, `~/.gnupg`, `~/.aictl`, `~/.aws`, `~/.config/gcloud`, `/etc/shadow`, `/etc/sudoers`. **INFO**
- Null-byte rejection in `check_path` (`security.rs:578`). **INFO**
- **TOCTOU window** between `check_path_write` (canonicalize parent) and `tokio::fs::write`/`edit_file`: a local attacker who can create a symlink at the target filename between validation and write can redirect the write outside the jail. Exploitation requires local write to the CWD or its parent. **LOW**
- `list_directory` labels symlinks for display (`tools.rs:204`) but file read/write tools follow symlinks transparently; canonicalize mitigates existing-target cases but not write-new-file cases where the final component is a symlink created by another process. **LOW**
- `write_file` has a configurable size cap (`AICTL_SECURITY_MAX_WRITE`, default 1 MB). **INFO**

## Error Handling & Info Leaks
- 64 `unwrap()/expect()/panic!` occurrences across `src/`; most are in tests or one-shot CLI bootstrap paths. `Mutex::lock().unwrap()` in `agents.rs` and `session.rs` is the common panic source; poisoning is unlikely but would crash the REPL. **LOW**
- Error messages propagate raw `reqwest`/`serde_json` strings (e.g. `llm_anthropic.rs:176` includes response body in the error). If any provider echoes the request headers (`x-api-key` / `Authorization`) in an error envelope, the key could surface in user-visible errors across 9 providers. **LOW**
- No stack traces or file paths are exposed in release-mode errors beyond user-supplied paths. **INFO**

## Denial of Service
- LLM responses are bounded by `MAX_RESPONSE_TOKENS = 4096`; agent loop caps at `MAX_ITERATIONS = 20` per turn (`config.rs:14-17`). **INFO (strength)**
- Tool output truncated to `MAX_TOOL_OUTPUT_LEN = 10_000` bytes with UTF-8 boundary walk-back (`tools.rs:93-102`). **INFO**
- **`fetch_url` / `extract_website` read full response body** before truncation — a hostile page can exhaust memory. **MEDIUM**
- **No global HTTP timeout** — hanging calls only cancellable via Esc in the REPL. Single-shot (`-m`) and `--auto --quiet` have no cancel path. **MEDIUM**
- Shell subprocess timeout (default 30s) is configurable and enforced via `tokio::time::timeout`. **INFO**
- `search_files_blocking` walks `**/*` and reads every file; blocks a single tokio task thread but output is capped at `MAX_TOOL_OUTPUT_LEN`. **LOW**

## Summary

Overall posture: **7 / 10**. The security model is well-designed for a tool that executes LLM-driven shell/file commands: a CWD jail, canonicalized path checks, command blocklist, env scrubbing, subshell blocking, prompt-injection detection, and an OS-keyring secret store are all wired in and exercised by tests. No `unsafe` code, clean `cargo audit`, no secrets in logs. The main gaps are around **URL-fetching tools** (no SSRF guard, no body cap, no default HTTP timeout) and **file-mode hygiene** for the plain-text config, plus one **missed input validation** on `--agent` at the CLI edge.

**Critical (must fix)** — none.

**High (should fix)**
- SSRF via `fetch_url`/`extract_website`: block private IPs, link-local, cloud metadata (`169.254.169.254`), `file://`; enforce an allowlist of schemes and a max body size (`tools.rs:419-527`, `security.rs:328`).
- `config::config_set` should `chmod 0600` on `~/.aictl/config` after write (`config.rs:192`) so `/unlock-keys` and plain-text fallback leave the file user-only.

**Medium (should fix)**
- Validate `cli.agent` with `agents::is_valid_name` before `read_agent` at `main.rs:1211` to close the local-file-exfil path.
- Set a default `reqwest::Client` timeout in `config::http_client` (e.g. 120s); let call sites override for shorter deadlines (`config.rs:9`).
- Add an `MAX_FETCH_BYTES` cap when reading `fetch_url` / `extract_website` response bodies (`tools.rs:427, 477`).
- Switch `fetch_geolocation` off cleartext `http://ip-api.com` (`tools.rs:550-553`).
- Harden the `curl | sh` updater: verify a signed checksum or ship tarball hashes alongside the install script (`commands.rs:1285`).

**Low (nice to have)**
- Add confirmation to `--clear-keys` on the CLI path (`main.rs:1144`) to match `/clear-keys` in the REPL.
- Use `OpenOptions::new().create_new(true)` or an atomic `rename` in `write_file` to shrink the TOCTOU window.
- Scrub `Authorization` / `x-api-key` from provider error strings before surfacing (`llm_*.rs`).
- `/unlock-keys` should warn if the resulting config file would be world/group-readable.
