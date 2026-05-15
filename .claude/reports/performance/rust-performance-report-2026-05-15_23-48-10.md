# Performance Report -- 2026-05-15 23:48:10

## Build Metrics

- Release binary `aictl`: **18 MB** (`__TEXT` 14.5 MB)
- Release binary `aictl-server`: **12 MB** (`__TEXT` 9.0 MB)
- Cold `cargo build --release` (workspace, 4 default-members minus desktop): **~1m 05s** wall, 327s CPU @ 5.4× parallelism
- Incremental release build: **~28 s**
- Release profile: **no customization** — workspace `Cargo.toml` has no `[profile.release]` block. Defaults: `lto = false`, `codegen-units = 16`, `debug = false`, `strip = false`. (`Cargo.toml:1-31`)
- `tokio = { features = ["full"] }` in **all three crates** (`crates/aictl-core/Cargo.toml:18`, `crates/aictl-cli/Cargo.toml:27`, `crates/aictl-server/Cargo.toml:24`).

## Allocations & Cloning

- **MEDIUM** `crates/aictl-core/src/llm/stream.rs:177-213` `split_pending` allocates **two new `String`s on every streamed delta** (the `flush` and `keep` halves). On a busy stream this fires per-token. Returning `&str` slices into the borrowed input or using `std::mem::take` on the keep half (then assigning the flush half into `pending` directly) would drop the per-token allocations.
- **MEDIUM** `crates/aictl-core/src/llm/openai.rs:99-105` (and the parallel paths in `gemini.rs:79-102`, `grok.rs:94-106`, `mistral.rs:85-93`, `zai.rs:94-98`, `deepseek.rs`, `kimi.rs`, `server_proxy.rs:108`) — `build_messages` calls `m.content.clone()` for every message on every LLM request. For long histories (system prompt + repo context + many tool results) this is a meaningful copy on the request hot path. The provider request types could borrow with `Cow<'a, str>` / `&'a str` and `serde` will still serialize correctly.
- **LOW** `crates/aictl-core/src/run.rs:2080` `let mut summary_msgs = messages.clone();` — full conversation clone before compaction. Compaction is rare so acceptable, but you could move-then-restore.
- **LOW** `crates/aictl-core/src/run.rs:1147-1149` per-outcome `combined_images.extend(o.images.iter().cloned()); … dispatched_names.push(o.call.name.clone());` inside the parallel-tool join loop — acceptable since batches are bounded by `AICTL_CODING_PARALLEL_TOOLS_MAX` (default 4).
- **INFO** 280 `.clone()`, 1094 `.to_string()`, 1245 `format!` invocations across the workspace — most are in cold paths (CLI parsing, error messages, slash commands), but the density on the LLM request build path is the one that matters.
- **INFO** Only 40 `with_capacity` calls vs. 261 bare `Vec::new()` / `String::new()` — most are in cold paths but worth a sweep on per-turn / per-token hot spots.

## String Handling

- **MEDIUM** `crates/aictl-core/src/tools/filesystem.rs:388` — `line.to_lowercase().contains(&pattern_lower)` allocates a **new String per scanned line** when smart-case or insensitive-case search fires on the rg-fallback path. For a directory with thousands of files this is wasted heap churn. A case-insensitive `regex` (built once outside the file loop) or `eq_ignore_ascii_case` over byte windows would avoid it.
- **LOW** `crates/aictl-core/src/llm/openai.rs:94-98` — `role` becomes `String::from("system" | "user" | "assistant")` per message; the field could be `&'static str` for serialization since the values are constants.
- **LOW** `crates/aictl-core/src/security.rs:1117-1118` — `let upper = key.to_uppercase()` for every env var on every shell call inside `scrubbed_env`. Could short-circuit with `key.contains_ignore_ascii_case` style helper or by using `eq_ignore_ascii_case` slot-by-slot. Per-call but env vars typically < 100.
- **INFO** A handful of `.to_lowercase()` calls in cold paths (`commands/model.rs:160`, `commands/history.rs:128`, REPL provider label formatting) — fine to leave.

## Async & Concurrency

- **STRENGTH** Tool dispatch already parallelizes read-only batches via `tokio::task::JoinSet` (`crates/aictl-core/src/run.rs:1112`) capped by `AICTL_CODING_PARALLEL_TOOLS_MAX`.
- **STRENGTH** MCP server initialization fans out with `futures_util::future::join_all` (`crates/aictl-core/src/mcp.rs:154`) — startup is bounded by the slowest server, not the sum.
- **STRENGTH** `/ping` and `/balance` probes parallelize across providers (`crates/aictl-cli/src/commands/ping.rs:70-71`, `crates/aictl-core/src/llm/balance.rs:88`).
- **INFO** `crates/aictl-core/src/mcp/stdio.rs:97-98` uses `tokio::sync::Mutex` for the JSON-RPC pending map. Briefly held; pending insert/remove couldn't easily use `std::sync::Mutex` because of cross-await holding inside `request`, but the current shape is fine.
- **INFO** `crates/aictl-core/src/run.rs:588-608` `Arc<Mutex<StreamState>>` is locked once per delta in the streaming sink closure. Lock is uncontended (single producer, single consumer), so `std::sync::Mutex` is the right choice here.
- **INFO** No `std::thread::sleep` or `std::fs::*` calls inside async hot paths. Filesystem-bound tool operations are routed through `tokio::fs::*` (filesystem, json_query, csv_query, image, diff, lint, mlx download) or `spawn_blocking` (checksum, search/find rg-fallback, pdf/docx/spreadsheet parsing, host resolution in `check_port`).

## I/O & Network

- **STRENGTH** Single shared `reqwest::Client` via `config::http_client()` (`crates/aictl-core/src/config.rs:47-49`) cached in a `OnceLock`. Every provider call reuses it (`grep` shows 19 call sites all going through this helper, no per-request `Client::new()` outside MCP HTTP/SSE clients which legitimately need their own builders).
- **STRENGTH** Provider calls wrapped in `tokio::time::timeout(llm_timeout, …)` (`crates/aictl-core/src/run.rs:1547`); MCP requests wrapped per-RPC; subprocess execution wrapped in `shell_timeout`.
- **MEDIUM** `crates/aictl-core/src/tools/filesystem.rs:380` — `std::fs::read_to_string(&path)` in `search_files_fallback` reads each file fully into memory before line iteration. Inside `spawn_blocking` so the runtime isn't blocked, but for a workspace with large generated files this can spike memory. A `BufReader::lines()` walk would stream. Note this is only the no-`rg` fallback; the `rg` shellout is the fast path.
- **LOW** `crates/aictl-core/src/tools/filesystem.rs:113` — `tool_read_file` uses `tokio::fs::read_to_string` and only afterward applies a line range. For a multi-MB file where only lines 5-10 are requested, the whole file is still read. Could `BufReader::lines()` early-exit at the upper line bound.
- **INFO** Subprocess output capture uses `output()` which reads stdout/stderr fully; bounded by `truncate_output` (`MAX_TOOL_OUTPUT_LEN = 10_000`) after the fact, not before — a runaway subprocess could allocate megabytes before truncation. Acceptable given `shell_timeout`.

## Process Execution

- **STRENGTH** `rg_available()` probes for ripgrep once with `OnceLock<bool>` (`crates/aictl-core/src/tools/filesystem.rs:12-26`) instead of re-spawning on every search.
- **STRENGTH** All long-running subprocesses (`shell.rs`, `coding.rs:665`, `test.rs:87`, `lint.rs:359`, `git.rs:335`, `system_info.rs:121`, `plugins.rs:551`, `notify.rs:46`) honor `security::shell_timeout`.
- **STRENGTH** Plugins spawn the entrypoint directly (no shell), pipe input on stdin, and use `kill_on_drop(true)` semantics via the security gate.
- **INFO** Subprocess stdout/stderr captured with `String::from_utf8_lossy(...).into_owned()` then truncated post hoc — fine.

## Data Structures & Algorithms

- **INFO** `crates/aictl-core/src/llm.rs:25-30` `provider_for_model` does a linear scan over a 70-entry `&[(&str, &str, &str)]`. Called rarely (per turn, not per token), so HashMap conversion would be theatrical.
- **INFO** `crates/aictl-core/src/security.rs:1098` `SAFE_ENV_VARS.contains(&key.as_str())` is a linear scan over a small `&[&str]` — fine.
- **INFO** All redaction regexes compile once via `OnceLock` in `crates/aictl-core/src/security/redaction.rs:1236-1471` and in `crates/aictl-cli/src/ui.rs:299-304`. No regex compiled inside a loop in production code (the only `Regex::new` in a hot path is `search_files_fallback` at `filesystem.rs:360`, which compiles **once per call** — correct since the pattern is user-supplied per invocation).
- **MEDIUM** `crates/aictl-core/src/run.rs:712-769` `redact_outbound` re-scans **every message** every turn against every detector. The system prompt and earlier turn contents do not change between iterations; a content-hash → result cache (or scanning only newly-appended messages) would cut the per-turn redaction cost in long sessions. Each detector pass is `O(message_size)`, and there are ~15 detectors plus the URL-span filter — 15× redundant work per old message per iteration.
- **INFO** Two loops nested in `redaction::find_matches` and `apply_placeholders` are unavoidable (regex scan + interval merge). The `merge_overlaps` call sorts after collection; for typical match counts this is negligible.

## Binary Size

- **HIGH** No `[profile.release]` overrides. Setting `lto = "thin"` typically cuts 15-25% off a Rust binary this size; `strip = true` removes symbol tables (another 1-3 MB on macOS); `codegen-units = 1` adds compile time but improves inlining. Estimated win: **3-5 MB** off `aictl`, **2-3 MB** off `aictl-server`.
- **MEDIUM** `tokio = { features = ["full"] }` everywhere. The server only needs `rt-multi-thread`, `net`, `time`, `sync`, `signal`, `macros`, `io-util`; it has no filesystem, process, or signal-extended needs that aren't covered by those. Trimming to a minimal feature set removes `tokio-process`, `tokio-fs`, `tokio-signal` codepaths from server crate metadata and shaves both compile time and final binary.
- **MEDIUM** `axum = { features = ["http1", "json", "tokio", "tower-log", "matched-path"] }` is already trimmed — good. But `tracing-subscriber = { features = ["json", "fmt", "env-filter", "ansi"] }` pulls a sizable parser tree; `json` and `ansi` are only useful in the structured-log path. Consider gating them behind an `aictl-server` feature.
- **LOW** 169 `#[derive(Debug)]` derives across the workspace. Debug formatting code lives in `__TEXT` even when not invoked at runtime. Removing Debug from large internal types (e.g. `RedactionPolicy`, `Cli` struct, MCP types, the OpenAI request structs) would trim a few hundred KB. Often easier to flip with a `#[cfg_attr(debug_assertions, derive(Debug))]` blanket — but Debug is sometimes part of the public API.
- **INFO** `pdf-extract`, `calamine`, `zip`, `tar`, `flate2`, `scraper`, `keyring`, `llama-cpp-2` (when on), `gline-rs` (when on), `mlx-rs` (when on) all add weight. They're all justifiable for the tool surface. `pdf-extract` is the heaviest pull for non-feature-gated functionality; consider feature-gating the `read_document` tool's PDF path if you want to ship a lighter default build.
- **INFO** `keyring v3` with `apple-native` + `sync-secret-service` features is correct for cross-platform — leave it.

## Startup & Responsiveness

- **STRENGTH** `MCP::init` is gated behind `AICTL_MCP_ENABLED=true` (default off) — third-party processes don't auto-spawn on startup.
- **STRENGTH** `plugins::init` and `hooks::init` are also gated.
- **STRENGTH** Configuration cached in `OnceLock<RwLock<HashMap>>` (`config.rs:5-49`) — single disk read on first access.
- **STRENGTH** `audit::set_file_override`, `security::init`, MCP catalogue load, and the CLI argument parse all happen serially **before** the first user-visible output, but each is in the millisecond range.
- **LOW** `apply_cwd_override` runs `std::fs::canonicalize` (sync) before any TTY output — fine, but if the path is on a slow network volume the user has no feedback. Acceptable for now.
- **INFO** `fetch_remote_version` (`crates/aictl-cli/src/main.rs:28-47`) has a 3-second `.timeout(...)` — good, doesn't block startup. The `version_cache` reads avoid the network on hot path.
- **INFO** `rg_available()` first probe inside the first `search_files` / `find_files` call is one fork+exec. Cheap and one-time per process.

## Summary

**Overall score: 7.5 / 10.** This is a fundamentally well-architected codebase for performance: single shared HTTP client, OnceLock-cached regexes, parallel MCP startup, parallel tool batching with explicit caps, subprocess timeouts everywhere, file-read truncation, no obvious lock contention, no blocking syscalls inside async hot paths. The biggest losses are in **build-time / binary-size** (no release-profile optimization) and **a few allocation hot spots on the streaming and request-build path** that compound on long sessions.

**Critical issues:** none.

**High-impact:**
- Missing `[profile.release]` in workspace `Cargo.toml`: adding `lto = "thin"`, `strip = true`, `codegen-units = 1` is a one-line change with measurable binary-size win (~3-5 MB) and small startup-time gain.

**Medium-impact:**
- `split_pending` in `crates/aictl-core/src/llm/stream.rs:177-213` allocates two Strings per streamed delta — convertible to slice-based rotation.
- `build_messages` and parallel provider request builders clone full message content per request (`llm/openai.rs:99-105` and 5 other providers) — `Cow<'a, str>` would eliminate the per-message copy.
- `search_files_fallback` lowercases per line inside the file loop (`tools/filesystem.rs:388`) and reads each file fully into memory before scanning (`tools/filesystem.rs:380`).
- `redact_outbound` re-runs every detector against every message on every loop iteration (`run.rs:712-769`) — a hash-keyed result cache would scale better in long sessions.
- `tokio = "full"` in all three crates pulls features none of them actually need.

**Low-impact / Suggestions:**
- Per-line `to_lowercase` allocations in `scrubbed_env` env-var classification.
- Full conversation clone in `compact_messages`.
- Trimming `Debug` derive on large internal types.
- Gating `tracing-subscriber` `json` / `ansi` features behind an `aictl-server` feature.
