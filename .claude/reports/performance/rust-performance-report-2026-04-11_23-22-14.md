# Performance Report — 2026-04-11 23:22:14

## Build Metrics
- Binary size: **9.7 MB** (`target/release/aictl`, x86_64 darwin).
- Clean release build: **28.86s** (157.9s user on 6-core).
- `Cargo.toml`: 11 direct dependencies.
- **No `[profile.release]` section** — default release settings (no LTO, no `strip`, default `codegen-units`, `panic = unwind`).
- `tokio = { features = ["full"] }` — pulls the entire runtime even though only a subset is used.

## Allocations & Cloning
- **CRITICAL** `src/main.rs:362-365` — `messages.clone()` clones the entire conversation history on every agent-loop iteration in `MemoryMode::LongTerm`. Up to 20 clones per user turn; each clone duplicates every `String` content. Providers already take `&[Message]`, so `messages.as_slice()` would avoid the clone entirely.
- **HIGH** per-message content clones in every provider: `src/llm_openai.rs:58`, `src/llm_anthropic.rs:95/100/106/127/129`, `src/llm_gemini.rs:69/75/83`, `src/llm_grok.rs:58`, `src/llm_mistral.rs:50`, `src/llm_deepseek.rs:55`, `src/llm_kimi.rs:60`, `src/llm_zai.rs:50`, `src/llm_ollama.rs:89`. Each call rebuilds a request struct cloning every `m.content` into a new `String`. Serde supports borrowed `&str` fields (or `Cow<'_, str>`); this would halve per-call allocations.
- **MEDIUM** `src/commands.rs:193` — compaction clones the full `messages` vec before pushing one extra message. Could pass a slice + tail instead.
- **LOW** `src/llm_anthropic.rs:127-131` (`mark_cached`) — clones text out of an existing `MessageContent::Text` to rewrap into `Blocks`. `std::mem::replace` would move ownership.
- **INFO** `src/tools.rs:430/442/502` — HTML stripping pre-allocates with `String::with_capacity(body.len())`.

## String Handling
- **MEDIUM** Every provider request struct owns `model: String` (`src/llm_openai.rs:8`, `src/llm_anthropic.rs:11`, etc.) forcing `model.to_string()` at the call site even though `&'a str` would serialize identically.
- **LOW** `src/security.rs:821` — `input.to_lowercase()` clones the whole user input for case-insensitive pattern scan. Only runs once per user turn, but a case-insensitive ASCII scan would be allocation-free.
- **LOW** `src/session.rs:56, 201, 204` — `.to_string()` in name-map parsing; one allocation per line read. Fine at current scale.

## Async & Concurrency
- **HIGH** `src/tools.rs:469-527` (`tool_extract_website`) — `scraper::Html::parse_document` and the full descendants walk run inline on the async task. On a large HTML page this blocks the runtime worker. Wrap in `tokio::task::spawn_blocking` the same way `search_files_blocking` is (`src/tools.rs:228`).
- **MEDIUM** `src/tools.rs:419-467` (`tool_fetch_url`) — the char-by-char HTML strip and whitespace collapse are two serial passes over the entire page body inside the async task. For large responses, wrap in `spawn_blocking`.
- **MEDIUM** `src/session.rs:177-187` (`save_messages`) — called from the async REPL loop (`src/main.rs:955, src/main.rs:1043`). Uses blocking `std::fs::write` + `serde_json::to_string_pretty` of the whole history after every turn. For long sessions this stalls the runtime between turns. Use `tokio::fs::write` or `spawn_blocking`.
- **INFO** `src/tools.rs:228` — `search_files_blocking` correctly uses `spawn_blocking`.
- **INFO** `src/config.rs:8-10` — shared `reqwest::Client` via `OnceLock` is correct (connection reuse, TLS session caching).

## I/O & Network
- **HIGH** `src/main.rs:914` (`run_interactive`) — startup `await`s `fetch_remote_version()` **before** showing the welcome banner. That call has a 3s timeout (`src/main.rs:65`), so the REPL can stall up to 3s on every launch just for a "(vX available)" notice. Spawn it in parallel with the rest of startup, or defer to the first idle moment.
- **HIGH** `src/config.rs:8-10` — shared client has **no default timeout**. LLM calls rely on `with_esc_cancel` for user cancellation but can otherwise hang; tool calls (`src/tools.rs:322, 422, 472, 557`) have no timeout protection at all. Set `reqwest::ClientBuilder::timeout(..)` in `http_client()`.
- **MEDIUM** `src/tools.rs:245` — `std::fs::read_to_string(&path)` in `search_files_blocking` reads whole files into memory. Use `BufReader::read_line` for line-by-line scanning and short-circuit on binary content via a NUL-byte probe.
- **LOW** `src/config.rs:173, 210` — `config_set` / `config_unset` re-read and rewrite the whole config file on every mutation. Acceptable given config is small and mutations are rare.

## Process Execution
- **MEDIUM** `src/tools.rs:530` (`tool_fetch_datetime`) — forks the `date` subprocess to get the current time. `std::time::SystemTime` + manual formatting avoids the subprocess entirely. The system prompt instructs the LLM to call this tool first for any relative-time query, so it's a hot path.
- **INFO** `src/tools.rs:114-122` — `exec_shell` wraps `cmd.output()` in `tokio::time::timeout` and environment-scrubs before spawning. Correct.

## Data Structures & Algorithms
- **HIGH** `src/session.rs:234-239` (`list_sessions`) — **O(N²)**: for each directory entry it calls `name_for(&fname)` (`src/session.rs:236`), and `name_for` (`src/session.rs:72-77`) re-reads and re-parses the full `.names` file once per entry. Read the names file once into a `HashMap<String, String>` before the loop.
- **MEDIUM** `src/security.rs:630-656` (`validate_path`) — for every file tool call it re-`canonicalize()`s each `blocked_path`, the CWD (`pol.working_dir.canonicalize()` at 645), and every `allowed_path`. These are stable after `load_policy()`; canonicalize once at init and store results in the policy struct. Filesystem syscalls on every tool call.
- **MEDIUM** `src/tools.rs:110`, `src/security.rs:689-720` (`scrubbed_env`) — recomputes the allowed-env list on every `exec_shell` invocation (walks `std::env::vars()`, per-var uppercase, `iter().any` scans). Process env is stable; materialize once in `SecurityPolicy`.
- **LOW** `src/security.rs:262, 365, 372, 704` — linear `iter().any()` over `disabled_tools`, `blocked_commands`, `allowed_commands`, `blocked_env_vars` on every validation. Lists are small today; a `HashSet` is not mandatory, flag for growth.

## Binary Size
- **MEDIUM** `Cargo.toml` — no `[profile.release]`. Adding `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"` typically trims a Rust CLI like this by 30-50% (from 9.7 MB toward ~5-6 MB) at the cost of longer link time. Measure before committing.
- **LOW** `Cargo.toml:15` — `tokio = ["full"]`. You don't need `signal`, the entire `net` beyond what `reqwest` pulls, or `sync::broadcast`. Narrow to `["rt-multi-thread", "macros", "process", "fs", "time", "io-util", "sync"]` and verify `with_esc_cancel` still builds (needs `spawn_blocking`, `oneshot`).
- **LOW** `Cargo.toml:21` — `scraper` is only used in `tool_extract_website`. It pulls `html5ever`, `cssparser`, `selectors`, `phf`, etc. If extraction is rare, consider a hand-rolled text extractor to drop the dep.
- **INFO** No unused `#[derive(Debug)]` spotted on hot-path structs.

## Startup & Responsiveness
- **HIGH** `src/main.rs:914` — `fetch_remote_version().await` runs before `InteractiveUI::print_welcome`; user waits up to 3s for the banner. Biggest user-perceived latency. Fix: `tokio::spawn` it and, when printing, check if it has completed; otherwise print with an empty version_info.
- **MEDIUM** `src/main.rs:1088` — `load_config()` synchronously reads `~/.aictl/config` before `Cli::parse()`. Fast (config is small) but precedes argv parsing; reversing the order lets `--version`/`--help` bail earlier without touching disk.
- **LOW** `src/main.rs:968-973` — `rl.load_history()` reads `~/.aictl/history` synchronously after the welcome banner. Fine; rustyline is fast.
- **INFO** `src/config.rs:4-5`, `src/security.rs:153` — `OnceLock` lazy init is used correctly; no heavy eager init at startup other than the version check.

## Strengths
- Single shared `reqwest::Client` via `OnceLock` — correct HTTP connection reuse.
- `OnceLock<RwLock<HashMap>>` for config avoids per-call env reads.
- `spawn_blocking` used correctly for recursive filesystem search.
- Shell subprocess has env scrubbing **and** a configurable timeout.
- UTF-8 boundary-safe truncation in `truncate_output` (`src/tools.rs:93-102`).
- Output size caps via `MAX_TOOL_OUTPUT_LEN` short-circuit large results.
- Anthropic provider uses prompt caching with stable + rolling breakpoints — significant token-cost savings between agent-loop iterations.

## Summary
**Score: 7/10** — correct and well-structured, but has one structural allocation waste in the agent loop plus a handful of fixes that would noticeably improve startup and per-turn latency.

**Critical issues (measurable impact)**
1. `src/main.rs:363` — `messages.clone()` per iteration in LongTerm memory mode; O(N·iterations) bytes cloned every turn.
2. `src/main.rs:914` — blocking remote-version fetch delays the welcome banner by up to 3s at every REPL start.

**Warnings (likely impact)**
3. `src/session.rs:236` — O(N²) `list_sessions` due to repeated name-file reads.
4. `src/tools.rs:469` — `tool_extract_website` runs DOM parsing inline on the async task.
5. `src/session.rs:177` — blocking `fs::write` + pretty-print of whole history after every turn.
6. `src/security.rs:630, 645, 650` — repeated `canonicalize()` syscalls on every file tool call.
7. `src/llm_*.rs` — per-message `content.clone()` in every provider.
8. `src/config.rs:8-10` — `http_client()` has no default timeout; tool URL fetches can hang indefinitely.

**Suggestions (marginal improvement)**
9. Add `[profile.release]` with `lto`, `strip`, `codegen-units = 1` to trim the 9.7 MB binary.
10. Replace `tokio = ["full"]` with a scoped feature set.
11. Replace `tool_fetch_datetime`'s `date` subprocess with in-process time formatting.
12. Cache `scrubbed_env()` once in policy init.
13. Borrow `&str` fields in provider request structs instead of owning `String`.
