# Architecture

## Module Structure

Two-crate Cargo workspace: `crates/cli/` (binary, package `cli`, produces the `aictl` executable) and `crates/engine/` (library, package `engine`). The CLI depends on the engine via a path dependency and re-exports its modules under `crate::*` for legacy import paths. Frontend deps (`crossterm`, `rustyline`, `termimad`, `indicatif`) only live in the CLI; the engine never names a terminal type.

```
crates/cli/src/
 ├── main.rs            CLI args (clap) and management-flag dispatch (--version, --update, --uninstall, --config, agent/skill/session/key/model helpers, --audit-file), provider/model/key resolution, then routes to single-shot or REPL. Re-exports engine modules (config, llm, run, security, …) under crate::* so legacy paths in REPL / slash-command code keep resolving.
 ├── repl.rs            Interactive REPL driver — reads input, dispatches slash commands, drives run_agent_turn, persists session/stats after each turn
 ├── commands.rs        REPL slash-command dispatch + CommandResult enum (/agent, /balance, /behavior, /clear, /compact, /config, /context, /copy, /exit, /gguf, /help, /history, /hooks, /info, /keys, /mcp, /memory, /mlx, /model, /ping, /plugins, /retry, /roadmap, /security, /session, /skills, /stats, /tools, /undo, /uninstall, /update, /version); unrecognized /<name> falls through to skills::find for user-authored skill invocation
 ├── commands/          One submodule per slash command (agent, balance, behavior, clipboard, compact, config_wizard, gguf, help, history, hooks, info, keys, mcp, memory, menu, mlx, model, ping, plugins, retry, roadmap, security, session, skills, stats, tools, undo, uninstall, update). MemoryMode is re-exported from engine::run.
 ├── ui.rs              PlainUI (single-shot, pipe-friendly) + InteractiveUI (REPL: termimad markdown, crossterm tool-confirm selector, indicatif progress backend, raw-mode Esc cancel listener). Re-exports engine::ui::{AgentUI, ProgressHandle, ProgressBackend, ToolApproval} so legacy crate::ui::AgentUI paths keep resolving.
 └── version_cache.rs   Cached remote-version lookup under ~/.aictl/version (TTL gating /version & banner staleness check)

crates/engine/src/
 ├── lib.rs             pub mod declarations + an explicit pub use re-export block (Message, Role, ImageData, Provider, Interrupted, with_esc_cancel, build_system_prompt, run_agent_single, AgentUI, ToolApproval, ProgressHandle, ProgressBackend, WarningSink, AictlError); engine VERSION constant for User-Agent / MCP clientInfo
 ├── run.rs             run_agent_turn loop, tool-call dispatch, outbound redaction, Provider enum, Esc-cancel wiring (uses AgentUI::interruption), build_system_prompt, run_agent_single (takes &dyn AgentUI), MemoryMode enum
 ├── message.rs         Message/Role/ImageData types shared across providers
 ├── error.rs           Crate-wide AictlError (thiserror); CLI maps rustyline errors manually so engine doesn't depend on the REPL stack
 ├── agents.rs          Agent prompt management (~/.aictl/agents/), loaded-agent state, CRUD, name validation
 ├── audit.rs           Per-session tool-call audit log (~/.aictl/audit/<session-id>, JSONL), AICTL_SECURITY_AUDIT_LOG toggle; --audit-file <PATH> via set_file_override redirects to an explicit path and force-enables logging for single-shot runs; also log_redaction() for the redaction layer's events
 ├── hooks.rs           User-defined lifecycle hooks loaded from ~/.aictl/hooks.json (override via AICTL_HOOKS_FILE). Eight events (SessionStart/End, UserPromptSubmit, PreToolUse, PostToolUse, Stop, PreCompact, Notification). Glob matcher (*, ?, |) over tool name. JSON payload on stdin; stdout JSON shapes (decision: block|approve, additionalContext, rewrittenPrompt) influence the harness; exit 2 = block. Default 60s timeout, scrubbed env, security CWD. --unrestricted does NOT bypass.
 ├── mcp.rs             Model Context Protocol client. Servers declared in ~/.aictl/mcp.json (override via AICTL_MCP_CONFIG); spawned at startup, JSON-RPC over stdio (Phase 1: stdio transport + tools only). Tools surface as mcp__<server>__<tool>; merged into the agent loop catalogue alongside built-ins and plugins. Master switch AICTL_MCP_ENABLED (default false) — third-party processes do not auto-spawn. Per-server failures land in ServerState::Failed and never abort startup.
 ├── mcp/               Submodules: config.rs (mcp.json parser + ${keyring:NAME} substitution + AICTL_MCP_TIMEOUT/STARTUP_TIMEOUT/DISABLED), protocol.rs (JSON-RPC envelope + initialize/tools/list/tools/call types), stdio.rs (StdioClient: spawn child, line-delimited JSON-RPC reader task, request/response correlation by id, kill_on_drop)
 ├── config.rs          Config file loading (~/.aictl/config) into RwLock-backed cache, constants (system prompt, spinner phrases, agent loop limits), project prompt file loading; load_config returns Result so the CLI can surface a HOME-missing error
 ├── keys.rs            Secure API key storage. System keyring (Keychain / Secret Service) with transparent plain-text fallback. lock_key/unlock_key/clear_key migration primitives.
 ├── plugins.rs         User plugin discovery + execution under ~/.aictl/plugins/<name>/ (override via AICTL_PLUGINS_DIR), gated behind AICTL_PLUGINS_ENABLED. Plugin tools surface alongside built-ins; security gate + scrubbed_env applied identically.
 ├── security.rs        SecurityPolicy, shell/path/env validation, CWD jail, timeout, output sanitization. init() returns redaction-policy warnings for the CLI to surface (engine never reaches for stderr directly).
 ├── security/redaction.rs        Outbound-message redactor. RedactionPolicy (off/redact/block), Layer A regex detectors (API keys, AWS, JWT, PEM private keys, connection strings, email, phone, credit cards via Luhn, IBAN via mod-97), Layer B Shannon-entropy scanner for opaque tokens, user-defined AICTL_REDACTION_EXTRA_PATTERNS, AICTL_REDACTION_ALLOW allowlist, overlap merging by priority.
 ├── security/redaction/ner.rs    [optional, redaction-ner feature] Layer C — gline-rs-backed NER model manager + inference. Management paths (list/remove/download_model, spec parsing, status) always compiled; GLiNER loading and span-mode inference gated behind the feature. Specs: owner/repo or hf:owner/repo (default: onnx-community/gliner_small-v2.1). Models live under ~/.aictl/models/ner/<name>/{tokenizer.json,onnx/model.onnx}. Runtime warnings route through engine::ui::warn_global.
 ├── session.rs         Session persistence (~/.aictl/sessions/), UUID v4 generation, JSON save/load, names file, incognito toggle
 ├── skills.rs          Skill storage (~/.aictl/skills/<name>/SKILL.md), frontmatter (name/description) parsing, CRUD, reserved-name guard, AICTL_SKILLS_DIR override. Skills are single-turn markdown playbooks merged into the base system prompt for one run_agent_turn call and never persisted into session history
 ├── tools.rs           XML tool-call parsing, tool execution dispatch (security gate + output sanitization), duplicate-call guard, TOOL_COUNT (31)
 ├── tools/             One submodule per tool (archive, calculate, check_port, checksum, clipboard, csv_query, datetime, diff, document, filesystem, geo, git, image, json_query, lint, list_processes, notify, run_code, shell, system_info, util, web)
 ├── ui.rs              AgentUI trait, ToolApproval, ProgressHandle + ProgressBackend (so the engine doesn't link indicatif), the WarningSink / set_warning_sink / warn_global global-warn surface. No terminal-library types in scope; concrete impls live in crates/cli/src/ui.rs.
 ├── stats.rs           Per-day usage statistics (~/.aictl/stats). record()/today()/this_month()/overall()/day_count()/clear_all() back the view and clear entries of the /stats menu.
 ├── llm.rs             TokenUsage type, cost estimation (price_per_million), MODELS list, context_limit, cache_read_multiplier
 └── llm/               One submodule per provider
     ├── openai.rs      OpenAI chat completions client
     ├── anthropic.rs   Anthropic messages client
     ├── gemini.rs      Google Gemini generateContent client
     ├── grok.rs        xAI Grok chat completions client
     ├── mistral.rs     Mistral chat completions client
     ├── deepseek.rs    DeepSeek chat completions client
     ├── kimi.rs        Kimi (Moonshot AI) chat completions client
     ├── zai.rs         Z.ai chat completions client
     ├── ollama.rs      Ollama local model client (dynamic model discovery via /api/tags)
     ├── gguf.rs        [experimental] Native GGUF inference + model manager (~/.aictl/models/gguf/). Download takes &dyn AgentUI for progress reporting via the trait's progress_* methods. Inference gated behind the `gguf` cargo feature (llama-cpp-2). Specs: hf:owner/repo/file.gguf, owner/repo:file.gguf, https:// URL.
     └── mlx.rs (+ mlx/) [experimental, macOS Apple Silicon only] Native MLX inference + model manager (~/.aictl/models/mlx/<name>/). Download takes &dyn AgentUI for progress reporting. Inference gated behind the `mlx` cargo feature (mlx-rs + tokenizers + minijinja + safetensors). Llama-family architectures only. Specs: mlx:owner/repo or owner/repo (Hugging Face mlx-community).
```

Cargo features (`gguf`, `mlx`, `redaction-ner`) live on the engine crate; the CLI declares them as `engine/<feature>` passthroughs so `cargo install --path crates/cli --features "..."` from the workspace root works unchanged.

## Startup Flow

```
 ┌──────────────────────────────────────────────────────────────────────────┐
 │  main()                                                                  │
 │                                                                          │
 │  1. load_config()            read ~/.aictl/config into RwLock<HashMap>   │
 │  2. Cli::parse()             parse --provider, --model, -m, ...          │
 │  2b. security::init()        load SecurityPolicy into OnceLock           │
 │  2b'. plugins::init()        scan ~/.aictl/plugins/ when                 │
 │                              AICTL_PLUGINS_ENABLED=true; cache survivors │
 │  2b''. hooks::init()         parse ~/.aictl/hooks.json (override via    │
 │                              AICTL_HOOKS_FILE); cache hook table         │
 │  2b'''. mcp::init_with(only) spawn each enabled server in ~/.aictl/mcp. │
 │                              json (override via AICTL_MCP_CONFIG) when   │
 │                              AICTL_MCP_ENABLED=true; complete            │
 │                              `initialize` handshake under                │
 │                              AICTL_MCP_STARTUP_TIMEOUT (default 10s);    │
 │                              call tools/list and store the catalogue;    │
 │                              per-server failures recorded as             │
 │                              ServerState::Failed and skipped, never      │
 │                              fatal. `--mcp-server <name>` restricts to   │
 │                              one server without persisting config.       │
 │  2c. --list-sessions /       non-interactive session helpers, exit       │
 │      --clear-sessions                                                    │
 │  2c'. --list-agents          non-interactive agent listing, exit         │
 │       (+ --category <name> filter)                                       │
 │       --pull-agent <name>    pull official agent via agents::remote,     │
 │       (+ --force)            exit                                        │
 │  2c''. --pull-gguf-model /    GGUF model management helpers, exit        │
 │       --list-gguf-models /    (use llm::gguf::download_model / list /    │
 │       --remove-gguf-model /   remove_model / clear_models)               │
 │       --clear-gguf-models                                                │
 │  2c'''. --pull-mlx-model /    MLX model management helpers, exit         │
 │        --list-mlx-models /    (use llm::mlx::download_model / list /     │
 │        --remove-mlx-model /   remove_model / clear_models)               │
 │        --clear-mlx-models                                                │
 │  2c''''. --pull-ner-model /   NER model management helpers, exit         │
 │         --list-ner-models /   (use security::redaction::ner::...)        │
 │         --remove-ner-model /                                             │
 │         --clear-ner-models                                               │
 │  2d. --config                run_config_wizard() and exit                │
 │  3. resolve provider         flag > AICTL_PROVIDER config > error        │
 │  4. resolve model            flag > AICTL_MODEL config > error           │
 │  5. resolve api_key          keys::get_secret(LLM_*_API_KEY)             │
 │                              keyring first, plain-text config fallback   │
 │                              (Ollama / GGUF / MLX: no key needed)        │
 │  5b. session::set_incognito  --incognito flag or AICTL_INCOGNITO config  │
 │  5c. load --agent <name>    agents::read_agent + agents::load_agent      │
 │  6. dispatch:                                                            │
 │     ├─ -m given ──> run_agent_single()  (PlainUI)                        │
 │     └─ no -m ───> run_interactive()     (InteractiveUI + REPL)           │
 │                   ├─ load --session <id|name> or generate new uuid       │
 │                   │  (skipped when incognito)                            │
 │                   └─ print welcome banner (shows session or incognito)   │
 └──────────────────────────────────────────────────────────────────────────┘
```

## Agent Loop (`run_agent_turn`)

Both single-shot and REPL modes share the same loop:

```
 User message
      │
      ▼
 ┌─────────────────────────────────────────────────────────┐
 │  hooks::run_hooks(UserPromptSubmit) ── may block,       │
 │  rewrite prompt, or attach additionalContext            │
 │                                                         │
 │  security::detect_prompt_injection() ── block on match  │
 │  (guard; gated by AICTL_SECURITY_INJECTION_GUARD)       │
 │                                                         │
 │  Append user message to Vec<Message>                    │
 │                                                         │
 │  for _ in 0..MAX_ITERATIONS (20) {                      │
 │      │                                                  │
 │      ▼                                                  │
 │  ┌──────────────────────────────────────────────┐       │
 │  │ redact_outbound() — at the network boundary, │       │
 │  │ just before the provider call. Clones the    │       │
 │  │ message slice only when a credential / PII   │       │
 │  │ match is found; persisted history untouched. │       │
 │  │ Off by default (AICTL_SECURITY_REDACTION=    │       │
 │  │ off|redact|block); local providers bypass    │       │
 │  │ unless AICTL_SECURITY_REDACTION_LOCAL=true.  │       │
 │  │ Layer A regex + Layer B entropy + Layer C    │       │
 │  │ NER (optional `redaction-ner` feature).      │       │
 │  │ Block mode aborts the turn.                  │       │
 │  └──────────────────────────────────────────────┘       │
 │      │                                                  │
 │      ▼                                                  │
 │  ┌──────────────────┐                                   │
 │  │  Call LLM API    │  openai::call_openai()            │
 │  │  (via provider)  │  anthropic::call_anthropic()      │
 │  │                  │  gemini::call_gemini()            │
 │  │                  │  grok::call_grok()                │
 │  │                  │  mistral::call_mistral()          │
 │  │                  │  deepseek::call_deepseek()        │
 │  │                  │  kimi::call_kimi()                │
 │  │                  │  zai::call_zai()                  │
 │  │                  │  ollama::call_ollama()            │
 │  │                  │  gguf::call_gguf()                │
 │  │                  │  mlx::call_mlx()                  │
 │  └────────┬─────────┘                                   │
 │           │                                             │
 │           ▼                                             │
 │  ┌──────────────────┐                                   │
 │  │  parse_tool_call │  look for <tool name="...">       │
 │  └────────┬─────────┘                                   │
 │           │                                             │
 │       ┌───┴───┐                                         │
 │       │       │                                         │
 │    no tool  tool found                                  │
 │       │       │                                         │
 │       ▼       ▼                                         │
 │   Stop     hooks::run_hooks(PreToolUse) ──             │
 │   hook +   may block (deny) or pre-approve              │
 │   return       │                                        │
 │   answer       ▼                                        │
 │            ┌────────────────────┐                       │
 │            │  Confirm or --auto │                       │
 │            └────────┬───────────┘                       │
 │                 ┌───┴───┐                               │
 │              denied   approved                          │
 │                 │       │                               │
 │                 ▼       ▼                               │
 │          push deny   execute_tool()                     │
 │          message     push <tool_result> to messages     │
 │                 │       │                               │
 │                 │       ▼                               │
 │                 │   hooks::run_hooks(PostToolUse) ──   │
 │                 │   may attach <hook_context> turn      │
 │                 │       │                               │
 │                 └───┬───┘                               │
 │                     │                                   │
 │                     ▼                                   │
 │              next iteration ─────────────────────────>  │
 │  }                                                      │
 │                                                         │
 │  On final answer: hooks::run_hooks(Stop)                │
 │  may attach <hook_context> for the next turn            │
 └─────────────────────────────────────────────────────────┘
```

## Tool Execution (`execute_tool`)

```
 ┌───────────────────────────────────────────────────────────┐
 │  execute_tool(&ToolCall)                                  │
 │                                                           │
 │  1. security::validate_tool() ── deny? return error msg   │
 │  2. match tool_call.name:                                 │
 │  ┌─────────────────────┬───────────────────────────┐      │
 │  │ Tool                │ Backend                   │      │
 │  ├─────────────────────┼───────────────────────────┤      │
 │  │ exec_shell          │ sh -c (env scrub+timeout) │      │
 │  │ read_file           │ tokio::fs::read_to_string │      │
 │  │ write_file          │ tokio::fs::write          │      │
 │  │ remove_file         │ tokio::fs::remove_file    │      │
 │  │ create_directory    │ tokio::fs::create_dir_all │      │
 │  │ edit_file           │ read + replacen + write   │      │
 │  │ diff_files          │ in-process LCS unified    │      │
 │  │ list_directory      │ tokio::fs::read_dir       │      │
 │  │ search_files        │ glob + string match       │      │
 │  │ find_files          │ glob::glob                │      │
 │  │ search_web          │ Firecrawl API (reqwest)   │      │
 │  │ fetch_url           │ HTTP GET (reqwest)        │      │
 │  │ extract_website     │ HTTP GET + scraper (DOM)  │      │
 │  │ fetch_datetime      │ date command (subprocess) │      │
 │  │ fetch_geolocation   │ ip-api.com (reqwest)      │      │
 │  │ read_image          │ fs::read / HTTP GET+base64│      │
 │  │ generate_image      │ DALL-E/Imagen/Grok+write  │      │
 │  │ read_document       │ pdf-extract/zip/calamine  │      │
 │  │ git                 │ git subprocess (no shell) │      │
 │  │ run_code            │ interpreter via stdin     │      │
 │  │ lint_file           │ ext→linter (first on PATH)│      │
 │  │ json_query          │ jq filter (subprocess)    │      │
 │  │ csv_query           │ csv crate + SQL-like eval │      │
 │  │ calculate           │ recursive-descent eval    │      │
 │  │ list_processes      │ ps subprocess + parse     │      │
 │  │ check_port          │ tokio TcpStream::connect  │      │
 │  │ system_info         │ sysctl/vm_stat/df+/proc/* │      │
 │  │ archive             │ tar+flate2 / zip in-proc  │      │
 │  │ checksum            │ sha2/md-5 streaming digest│      │
 │  │ clipboard           │ pbcopy/wl-copy/xclip read │      │
 │  │ notify              │ osascript / notify-send   │      │
 │  └─────────────────────┴───────────────────────────┘      │
 │                                                           │
 │                                                           │
 │  3. sanitize_output() ── escape <tool> tags in results    │
 │  4. audit::log_tool() ── append JSONL entry to            │
 │     ~/.aictl/audit/<session-id> (executed, denied by      │
 │     policy/user, disabled, or duplicate)                  │
 │  All outputs truncated at 10,000 chars                    │
 │                                                           │
 │  Notes:                                                   │
 │  - read_image attaches ImageData to Message; providers    │
 │    encode it in their native vision format                │
 │  - generate_image auto-selects provider by available key: │
 │    active provider first, then OpenAI > Gemini > Grok     │
 │  - read_document dispatches by extension: .pdf via        │
 │    pdf-extract, .docx via zip + XML-to-markdown parser,   │
 │    .xlsx/.xls/.ods via calamine → markdown tables         │
 │  - git invokes `git` directly (no shell) with a strict    │
 │    per-subcommand flag allowlist and a scrubbed env that  │
 │    drops GIT_DIR / GIT_SSH_COMMAND / GIT_CONFIG_* etc.    │
 │  - run_code picks an interpreter (python/node/ruby/...)   │
 │    from the first line and pipes the rest of the snippet  │
 │    to stdin; kill_on_drop reaps the child on timeout      │
 │  - lint_file maps the file extension to an ordered list   │
 │    of candidate linters (rustfmt / ruff / eslint / ...)   │
 │    and runs the first one installed on PATH; no --fix     │
 │    flags are ever passed, so the file stays unchanged     │
 │  - json_query runs the filter via `jq` as a positional    │
 │    arg after `--` (no shell, no flag reinterpretation);   │
 │    JSON is piped on stdin or loaded from @path through    │
 │    the CWD jail. No -f / --slurpfile flags are passed     │
 │  - csv_query parses in-process via the `csv` crate with   │
 │    a tiny SQL-like evaluator (SELECT/FROM csv|tsv/WHERE/  │
 │    ORDER BY/LIMIT). Shares the @path security helper      │
 │    with json_query; renders results as a Markdown table   │
 │  - calculate evaluates math expressions via a recursive-  │
 │    descent parser (no eval, no shell). Supports operators,│
 │    parens, constants (pi/e/tau), one- and two-arg math    │
 │    functions; recursion depth is capped to stay safe      │
 │  - list_processes invokes `ps` directly (no shell) with   │
 │    LC_ALL=C for deterministic columns, parses rows in     │
 │    process, filters on name/user/pid/%cpu/%mem/port (port │
 │    resolved via `lsof`), and renders a Markdown table     │
 │  - check_port resolves DNS on spawn_blocking then runs    │
 │    TcpStream::connect inside tokio::time::timeout. Only   │
 │    completes the TCP handshake; reports classified errors │
 │    (refused, timed out, DNS failure, unreachable)         │
 │  - system_info renders OS/CPU/memory/disk as Markdown:    │
 │    macOS via sysctl + vm_stat + sw_vers + uname + df;     │
 │    Linux via /proc/cpuinfo + /proc/meminfo +              │
 │    /etc/os-release + df. Sections are filterable          │
 │  - diff_files computes an in-process LCS unified diff     │
 │    (3 lines of context) between two paths — no `diff`     │
 │    subprocess. Refuses files > 2000 lines each            │
 │  - archive create/extract/list handles tar.gz / tgz /     │
 │    tar / zip fully in-process via `tar`+`flate2`+`zip`.   │
 │    Extraction enforces a zip-slip / tar-slip guard and    │
 │    the CWD jail on every entry                            │
 │  - checksum streams the file through `sha2::Sha256` +     │
 │    `md-5::Md5` in chunks — arbitrary size, constant       │
 │    memory. `sha256 <path>` / `md5 <path>` picks one       │
 │  - clipboard picks a backend at runtime: pbcopy/pbpaste   │
 │    on macOS; wl-copy/wl-paste then xclip/xsel on Linux.   │
 │    Content piped on stdin; write capped at 1 MB           │
 │  - notify shells out to `osascript` on macOS or           │
 │    `notify-send` on Linux. Title required (≤256 B), body  │
 │    optional (≤4096 B). Useful for --auto completion pings │
 │  - tool names starting with `mcp__` route to             │
 │    mcp::call_tool: locate the (server, tool) pair in the  │
 │    cached catalogue, parse the JSON body, send tools/call │
 │    via the stdio JSON-RPC client, and return the          │
 │    concatenation of `content[]` text blocks. `[mcp error] │
 │    <reason>` on failure (timeout, decode, isError=true).  │
 │    Same security gate / audit / sanitization path as      │
 │    built-ins. See the MCP Servers section below           │
 │  - any other tool name falls through to plugins::find().  │
 │    User-installed plugin tools dispatch through the same  │
 │    security gate, audit, and sanitization path; see the   │
 │    Plugins section below for the manifest + wire protocol │
 └───────────────────────────────────────────────────────────┘
```

## Plugins (`crates/engine/src/plugins.rs`)

User-installed plugin tools live under `~/.aictl/plugins/<name>/` (override
via `AICTL_PLUGINS_DIR`) and let users add domain-specific tools without
forking the repo. Each plugin pairs a `plugin.toml` manifest with an
executable entrypoint (script or binary).

```
~/.aictl/plugins/
└── <name>/
    ├── plugin.toml
    └── run                # any executable, language-agnostic
```

`plugin.toml` fields:

- `name` — must match the directory name; re-validated at load.
- `description` — injected verbatim into the system-prompt tool catalog.
- `entrypoint` — relative path inside the plugin dir (default `run`);
  resolved + canonicalized; rejected if it resolves outside the plugin dir
  (symlink-escape guard) or is not executable on Unix.
- `requires_confirmation` (default `true`) — informational; the agent
  loop's existing y/N gate still owns the prompt.
- `timeout_secs` — per-plugin override of the global shell timeout.
- `schema_hint` — free-form text appended after the description in the
  catalog so the model knows the input shape.

Discovery (`init`) walks the directory at startup, skips entries that
collide with built-in tool names or appear in `AICTL_PLUGINS_DISABLED`,
logs (does not print) any malformed manifest, and stores the survivors
in a `OnceLock<Vec<Plugin>>`. A bad plugin is skipped, never fatal.

Wire protocol:

- **Input**: the raw `<tool>…</tool>` body on stdin (no JSON framing).
- **Output**: stdout returned verbatim (after `sanitize_output` neutralizes
  any embedded `<tool>` tags).
- **Exit code**: `0` → success; non-zero → `[exit N] <stderr>`.
- **Env**: `scrubbed_env()` (same helper `exec_shell` uses) — secrets/keys
  stripped.
- **CWD**: pinned to `security::policy().paths.working_dir` (the CWD jail).

Opt-in: `AICTL_PLUGINS_ENABLED=true` (default `false`). Plugins are
third-party code and must not auto-load. CLI surface: `--list-plugins`
for non-interactive listing; `/plugins` REPL menu for browse/manifest
view/master-switch toggle. The system-prompt catalog appends each plugin
under an `### <name> (plugin)` heading so the LLM can see it isn't
first-party.

Dispatch happens after the built-in match in `tools::execute_tool` —
`security::validate_tool` runs *before* the fallthrough, so
`AICTL_SECURITY_DISABLED_TOOLS` can disable plugin names exactly like
built-ins, the confirmation prompt fires unchanged, and `--unrestricted`
bypasses validation just as it does for built-ins.

## Hooks (`crates/engine/src/hooks.rs`)

User-defined shell commands the harness fires at lifecycle events. Hooks
are *harness* behavior, not LLM behavior: rules like "always run `cargo
fmt` after `edit_file`" or "block any `exec_shell` containing `git push`"
belong here, not in agent prompts or memory where the model can ignore
them.

Configured in `~/.aictl/hooks.json` (override the path with
`AICTL_HOOKS_FILE`). Top-level keys are event names; each value is an
array of `{ matcher, command, timeout, enabled }` entries. The parser
silently skips underscore-prefixed top-level keys (`_comment`, etc.) so
the file can carry inline JSON5-style notes.

```
~/.aictl/hooks.json:
{
  "PreToolUse": [
    { "matcher": "exec_shell", "command": "echo seen", "timeout": 30 }
  ],
  "PostToolUse": [
    { "matcher": "edit_file|write_file", "command": "cargo fmt --message-format short" }
  ],
  "Stop":       [ { "matcher": "*", "command": "..." } ]
}
```

Eight events, fired from these call sites:

| Event              | Where it fires                                          |
|--------------------|---------------------------------------------------------|
| `SessionStart`     | `repl::run_interactive` after session init; `run::run_agent_single` at entry |
| `SessionEnd`       | `repl::run_interactive` before exit banner; `run_agent_single` after answer  |
| `UserPromptSubmit` | `run::run_agent_turn` before injection guard            |
| `PreToolUse`       | `run::handle_tool_call` before y/N approval and `execute_tool` |
| `PostToolUse`      | `run::handle_tool_call` after the tool result joins history    |
| `Stop`             | `run::run_agent_turn` after the model returns a final answer   |
| `PreCompact`       | `commands::compact::compact` before the summary call    |
| `Notification`     | `tools::notify::tool_notify` before the OS-level pop    |

The matcher is a small glob (`*` = any run, `?` = single char, `|` =
alternation). For tool events the match target is the tool name; for
non-tool events the match target is empty so only `*` and empty patterns
hit. `glob_match` is implemented in `hooks.rs` directly to avoid a new
dependency.

Wire protocol:

- **stdin** — one JSON object: `{ event, session_id, cwd, tool: { name, input, output? }, prompt, notification, trigger, matcher }`. Only `event` is always present; the rest depend on the event kind.
- **stdout** — one of five shapes:
  - empty → `Continue`
  - `{ "decision": "block", "reason": "..." }` → abort the action; reason surfaced to the LLM as the tool result (or rejection error for prompt events)
  - `{ "decision": "approve", "reason": "..." }` → pre-approve a tool call (skips the user y/N prompt)
  - `{ "additionalContext": "..." }` → inject a `<hook_context>` user turn into history before the next LLM call
  - `{ "rewrittenPrompt": "..." }` → `UserPromptSubmit` only; replace the user's text before the agent sees it
  - plain text → treated as `additionalContext`
- **exit code** — `0` parses stdout normally; `2` is shorthand for `block` with stderr as the reason; any other non-zero exit is logged and treated as `Continue`.
- **env** — `scrubbed_env()` (same helper `exec_shell` and plugins use).
- **CWD** — pinned to `security::policy().paths.working_dir`.
- **timeout** — per-hook (default 60s); a timeout is logged and treated as `Continue` so a wedged hook can't hang the agent.

Outcomes from multiple matching hooks for the same event are folded into
a `HookOutcome`: a `block` from any hook wins; the first `approve` wins;
the first `rewrittenPrompt` wins; `additionalContext` strings accumulate
and are concatenated with a blank-line separator into a single
`<hook_context>` turn.

`--unrestricted` does **not** bypass hook execution. It only disables
the inner shell-validation that would otherwise refuse to run hook
commands containing blocked binaries — the hook itself always fires.

CLI surface: `--list-hooks` prints the catalogue; `/hooks` opens a REPL
menu (view all, toggle individual entries, test-fire with a synthetic
payload, reload from disk, show file path). Toggle/save round-trips go
through `hooks::save` + `hooks::replace` so changes take effect
mid-session without a restart.

## MCP Servers (`crates/engine/src/mcp.rs`)

Connect to external [Model Context Protocol](https://modelcontextprotocol.io)
servers and merge their tools into the agent loop alongside built-ins
and plugins. Phase 1 covers the **stdio** transport and **tools**
capability only — HTTP/SSE transport, resources, and prompts are on
the roadmap.

Servers are declared in `~/.aictl/mcp.json` (override via
`AICTL_MCP_CONFIG`) in a Claude Desktop-compatible shape:

```
{
  "mcpServers": {
    "<name>": {
      "command": "...",
      "args": [...],
      "env": { "K": "V" },
      "enabled": true,
      "timeout_secs": 30
    }
  }
}
```

Per-entry fields: `command` + `args` (resolved via `PATH`, no shell),
optional `env`, `enabled` (default `true`), `timeout_secs` (per-call
RPC timeout, falling back to `AICTL_MCP_TIMEOUT`, default 30s). Values
inside `env` may use `${keyring:NAME}` to pull a secret from
`keys::get_secret(NAME)` instead of checking it in.

Lifecycle (`mcp::init_with`):

1. Read the config; reject malformed entries (missing `command`,
   invalid name) — invalid names use the same alphanumeric +
   `_`/`-` rule as agents/skills/plugins.
2. For each enabled server (skipping anything in `AICTL_MCP_DISABLED`
   or excluded by `--mcp-server <only>`), spawn the child via
   `tokio::process::Command` with a scrubbed env + the entry's `env`
   overlay. `kill_on_drop(true)` is the backstop.
3. Wrap the `initialize` handshake in `tokio::time::timeout`
   (`AICTL_MCP_STARTUP_TIMEOUT`, default 10s) so a hung server cannot
   block startup.
4. On success send `notifications/initialized` and call `tools/list`;
   store the catalogue in a `OnceLock<Vec<McpServer>>`.
5. Any failure (spawn, handshake, list) lands in
   `ServerState::Failed(reason)`; the rest of the catalogue is
   unaffected.

All servers spawn in parallel via `futures_util::future::join_all`.

Wire protocol (`crates/engine/src/mcp/stdio.rs`):

- **Framing** — line-delimited JSON-RPC 2.0 (one envelope per line on
  stdin/stdout). The spec also describes a `Content-Length:` framing
  but it's rare in deployed servers; not implemented here.
- **Correlation** — each outbound request gets a monotonic integer
  `id`. A background reader task parses lines, looks up the matching
  `oneshot::Sender` in a `pending: Mutex<HashMap<i64, _>>`, and
  forwards the response. Lines that fail JSON parse (occasional
  startup banners) are silently dropped.
- **Stderr** — drained in a side task so the child can't block on a
  full pipe; not surfaced unless the user asks via `/mcp show`.

Catalog injection (`run::build_system_prompt`): every Ready server's
tools are appended under `### mcp__<server>__<tool> (mcp)` headings
with the description and pretty-printed JSON Schema, so the model can
self-format calls:

```
<tool name="mcp__filesystem__read_file">
{"path": "/tmp/notes.md"}
</tool>
```

Dispatch (`tools::execute_tool`): names starting with `mcp__` route to
`mcp::call_tool`. The body is parsed as JSON, sent as `tools/call`,
and the response's `content[]` text blocks are concatenated. Errors
surface as `[mcp error] <reason>` (matching the `[exit N]` convention
plugins use). The duplicate-call guard normalizes JSON bodies before
keying so whitespace differences don't create distinct entries.

Security gate:

- The `mcp__*` arm in `security::validate_tool` enforces a body-size
  cap (`max_file_write_bytes`) and `AICTL_MCP_DENY_SERVERS=foo,bar`,
  which blocks every tool from listed servers even when the master
  switch is on.
- `AICTL_SECURITY_DISABLED_TOOLS` accepts qualified names
  (`mcp__github__create_issue`).
- The CWD jail does **not** apply — MCP servers run in their own
  process with their own privileges. Users who want strict isolation
  should keep `AICTL_MCP_ENABLED=false` or curate the server list.
- Outbound redaction runs on the entire message stream regardless of
  transport.

Shutdown: `mcp::shutdown()` runs on every exit path (REPL `/exit`,
single-shot completion, error path). It best-effort-sends `shutdown`,
kills the child, and aborts the reader task. `kill_on_drop(true)` is
the safety net.

CLI / REPL surface: `aictl --list-mcp` for a non-interactive listing
(name, state, tool count, command); `aictl --mcp-server <name>`
restricts a single process to one server without persisting the
disable list. `/mcp` opens a REPL menu (view servers, browse per-tool
schemas, toggle the master switch, show config path). `/info` and the
welcome banner show MCP server / tool counts when enabled.

A bundled `tiny_add` smoke-test server lives at
`examples/mcp/tiny_add/server.py` with a fully-annotated example
config at `examples/mcp.json`.

## LLM Provider Abstraction

```
                             ┌──────────────┐
                             │  &[Message]  │
                             └──────┬───────┘
                                    │
               ┌────────────┬───────┼───────┬────────────┬────────────┬────────────┬────────────┬────────────┬────────────┐
               ▼            ▼       │       ▼            ▼            ▼            ▼            ▼            ▼            ▼
 ┌──────────────────┐ ┌───────────────────┐ ┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐ ┌──────────────────┐
 │  call_openai()   │ │ call_anthropic()  │ │  call_gemini()   │ │  call_grok()     │ │ call_mistral()   │ │ call_deepseek()  │ │  call_kimi()     │ │  call_zai()      │ │  call_ollama()   │
 │                  │ │                   │ │                  │ │                  │ │                  │ │                  │ │                  │ │                  │ │                  │
 │  System msg      │ │ System msg ──>    │ │ System msg ──>   │ │ System msg       │ │ System msg       │ │ System msg       │ │ System msg       │ │ System msg       │ │ System msg       │
 │  inline in       │ │ top-level         │ │ systemInstruction│ │ inline in        │ │ inline in        │ │ inline in        │ │ inline in        │ │ inline in        │ │ inline in        │
 │  messages[]      │ │ "system" field    │ │ field            │ │ messages[]       │ │ messages[]       │ │ messages[]       │ │ messages[]       │ │ messages[]       │ │ messages[]       │
 │                  │ │                   │ │                  │ │                  │ │                  │ │                  │ │                  │ │                  │ │                  │
 │  POST /v1/chat/  │ │ POST /v1/         │ │ POST /v1beta/    │ │ POST /v1/chat/   │ │ POST /v1/chat/   │ │ POST /chat/      │ │ POST /v1/chat/   │ │ POST /api/paas/  │ │ POST /api/chat   │
 │  completions     │ │ messages          │ │ :generateContent │ │ completions      │ │ completions      │ │ completions      │ │ completions      │ │ v4/chat/         │ │ (localhost:11434)│
 │  (openai.com)    │ │                   │ │                  │ │ (x.ai)           │ │ (mistral.ai)     │ │ (deepseek.com)   │ │ (moonshot.cn)    │ │ completions      │ │ no auth needed   │
 │                  │ │                   │ │                  │ │                  │ │                  │ │                  │ │                  │ │ (z.ai)           │ │                  │
 └────────┬─────────┘ └────────┬──────────┘ └────────┬─────────┘ └────────┬─────────┘ └────────┬─────────┘ └────────┬─────────┘ └────────┬─────────┘ └────────┬─────────┘ └────────┬─────────┘
          │                    │                     │                    │                    │                    │                    │                    │                    │
          └────────────────────┼─────────────────────┼────────────────────┼────────────────────┼────────────────────┼────────────────────┼────────────────────┼────────────────────┘
                               ▼                     │                    │                    │                    │                    │                    │
                    ┌──────────────────┐             │                    │                    │                    │                    │                    │
                    │ (String,         │ <───────────┴────────────────────┴────────────────────┴────────────────────┴────────────────────┴────────────────────┘
                    │  TokenUsage)     │
                    │                  │
                    │ response text +  │
                    │ input/output     │
                    │ token counts     │
                    └──────────────────┘
```

Two additional providers are not wired to remote endpoints. `call_gguf()` in `crates/engine/src/llm/gguf.rs` flattens `&[Message]` into a ChatML-style prompt and runs inference in-process via `llama-cpp-2` on a `tokio::spawn_blocking` task, loading a GGUF model from `~/.aictl/models/gguf/<name>.gguf`. It is compiled in only when the `gguf` cargo feature is enabled. `call_mlx()` in `crates/engine/src/llm/mlx.rs` builds a hand-written Llama-family transformer with `mlx-rs` primitives, renders the per-model jinja chat template via `minijinja` (ChatML fallback), loads safetensors shards from `~/.aictl/models/mlx/<name>/`, and runs greedy + temperature sampling with KV cache on a `tokio::spawn_blocking` task. It is compiled in only when the `mlx` cargo feature is enabled and only on macOS+aarch64; elsewhere the function returns an error telling the user to rebuild. Both report input/output token counts and cost always resolves to $0.00.

## UI Layer

```
               ┌─────────────┐
               │  AgentUI    │  trait
               │  (trait)    │
               └──────┬──────┘
                      │
            ┌─────────┴─────────┐
            ▼                   ▼
 ┌────────────────┐  ┌──────────────────┐
 │   PlainUI      │  │  InteractiveUI   │
 │                │  │                  │
 │  single-shot   │  │  REPL mode       │
 │  pipe-friendly │  │  spinner         │
 │  stdout/stderr │  │  colors          │
 │  no spinner    │  │  markdown render │
 │                │  │  tool box UI     │
 └────────────────┘  │  rustyline input │
                     │  command history │
                     └──────────────────┘
```

## REPL Command Dispatch (`commands.rs`)

```
 User input
      │
      ▼
 starts with '/'?
      │
  ┌───┴───┐
  no     yes
  │       │
  ▼       ▼
 send   commands::handle()
 to        │
 agent  ┌──┴────────┬───────────┬───────────┬───────────┐
 loop   ▼           ▼           ▼           ▼           ▼
      /exit       /clear      /compact    /copy       /help ...
      Exit        Clear       Compact     Continue    Continue
      (break)     (reset      (summarize  (pbcopy     (print
                  messages)   via LLM)    last_answer) commands)

 Also: /agent (Agent), /balance (Balance), /behavior (Behavior), /memory (Memory), /context (Context), /history (History), /hooks (Hooks), /info (Info), /gguf (Gguf), /mcp (Mcp), /mlx (Mlx), /ping (Ping), /plugins (Plugins), /security (Security), /session (Session), /skills (Skills), /model (Model), /tools (Continue), /stats (Stats), /keys (Keys), /config (Config), /retry (Retry), /roadmap (Roadmap), /undo (Undo), /update (Update), /uninstall (Uninstall), /version (Version). Any other /<name> the dispatcher doesn't recognize is tried as a skills::find lookup; on a hit it returns CommandResult::InvokeSkill, otherwise the "unknown command" error fires.

 CommandResult enum:
   Exit        → break REPL loop
   Clear       → reset messages & last_answer, continue
   Compact     → summarize conversation via LLM, save session, continue
   Agent       → open agent menu (create manually / create with AI / view all / unload);
                 loading/unloading rebuilds system prompt; continue
   Context     → show token/message usage, continue
   Info        → show provider/model/version/agent info, continue
   Security    → show security policy + per-key storage location, continue
   Session     → open session menu (current info / set name / view saved / clear all);
                 disabled in incognito mode; continue
   Skills      → open skills menu (create manually / create with AI / view all →
                 invoke / view / delete); on "invoke now" returns InvokeSkill so
                 the REPL loads the body and drives the next turn with it
   InvokeSkill → returned for /<skill-name> (or the menu's invoke action). The
                 REPL calls skills::find, passes Option<&Skill> into
                 run_agent_turn for exactly one turn, and reverts afterwards.
                 Task from "/<name> <task>" becomes the user message; when
                 absent, a default trigger ("Run the <name> skill.") fires so
                 the skill body alone drives the turn. Never persisted into
                 session history.
   Gguf        → open GGUF model menu (view downloaded / pull / remove / clear all);
                 downloads GGUF files to ~/.aictl/models/gguf/ with a progress bar; continue
   Mlx         → open MLX model menu (view downloaded / pull / remove / clear all);
                 downloads multi-file safetensors directories to ~/.aictl/models/mlx/<name>/
                 with a per-file progress bar; continue
   Ping        → probe every cloud provider catalog endpoint (`GET /models` with the
                 configured API key) plus the local Ollama daemon in parallel and
                 print per-provider status + latency; GGUF/MLX skipped (local only); continue
   Balance     → probe each cloud provider's balance endpoint (real for DeepSeek and
                 Kimi; "unknown + dashboard hint" elsewhere) and print remaining
                 credit / quota; local providers skipped; continue
   Plugins     → open plugins menu (list manifests, view a plugin's plugin.toml,
                 toggle the AICTL_PLUGINS_ENABLED master switch, show the plugins
                 directory); continue
   Hooks       → open hooks menu (view all hooks per event, toggle a hook on/off,
                 test-fire a hook with a synthetic payload, show hooks file path,
                 reload ~/.aictl/hooks.json); persists toggles to disk via
                 hooks::save + hooks::replace; continue
   Mcp         → open MCP servers menu (view all servers with state and tool
                 count, browse per-server tool catalogue with input schemas,
                 toggle the AICTL_MCP_ENABLED master switch, show mcp.json
                 config path); continue
   Stats       → open stats menu (view today/this-month/overall from ~/.aictl/stats /
                 clear all recorded usage statistics), continue
   Keys        → open keys menu (lock = config → keyring / unlock = keyring → config /
                 clear = remove from both, with confirmation), continue
   Update      → run update, restart if updated, continue
   Uninstall   → list install locations, ask y/N, delete the binary from
                 ~/.cargo/bin/, ~/.local/bin/, and $AICTL_INSTALL_DIR (if set);
                 break the REPL on success since the binary is gone, continue otherwise
   Version     → check current version against latest available, continue
   Config      → re-run interactive configuration wizard, continue
   Model       → select new model/provider, persist to ~/.aictl/config, continue
   Behavior    → switch auto/human-in-the-loop behavior, continue
   Memory      → switch memory mode (long-term/short-term), persist to ~/.aictl/config, continue
   Retry       → remove the last user/assistant exchange (skipping tool-result /
                 Tool-call-denied messages when locating the boundary), clear tool
                 call history and last_answer, save session, re-submit the removed
                 prompt via ReplAction::RunAgentTurnWith so the agent tries again
   History     → carries the raw `/history` arg string; the REPL filters the
                 in-memory conversation by role and/or keyword before printing
   Roadmap     → fetch and render the project ROADMAP.md; optional heading
                 filter (`/roadmap desktop`) jumps to the `## Desktop` section
   Undo        → carries a positive count; drop the last N turns without
                 re-running anything; refuses to cross a `/compact` boundary
   Continue    → command handled, continue
   NotACommand → pass input to agent loop (session saved after turn)
```

## Data Flow (end to end)

```
 User ──> CLI args / REPL input
           │
           ▼
      ┌──────────────┐
      │ commands.rs  │  (REPL only: slash command dispatch)
      └──────┬───────┘
             │ (not a command)
             ▼
      ┌──────────┐    ┌──────────────┐
      │ main.rs  │───>│  tools.rs    │
      │          │    │              │
      │ agent    │    │ parse_tool() │
      │ loop     │    │ execute_tool │
      │          │    └──────┬───────┘
      │          │           │
      │          │    ┌──────────────┐
      │          │    │ security.rs  │
      │          │    │ validate,    │
      │          │    │ env scrub,   │
      │          │    │ sanitize     │
      │          │    └──────────────┘
      │          │    ┌──────────────┐
      │          │───>│  config.rs   │
      │          │    │ SYSTEM_PROMPT│
      │          │    │ load_config  │
      │          │    └──────────────┘
      │          │    ┌──────────────┐
      │          │───>│  keys.rs     │
      │          │    │ get_secret   │
      │          │    │ keyring +    │
      │          │    │ plain-text   │
      │          │    │ fallback     │
      │          │    └──────────────┘
      │          │    ┌──────────────┐
      │          │───>│ agents.rs    │
      │          │    │ loaded_agent │
      │          │    │ save/load/   │
      │          │    │ delete/list  │
      │          │    └──────────────┘
      │          │    ┌──────────────┐
      │          │───>│ skills.rs    │
      │          │    │ find/list/   │
      │          │    │ save/delete, │
      │          │    │ frontmatter, │
      │          │    │ reserved     │
      │          │    │ name guard   │
      │          │    └──────────────┘
      │          │    ┌──────────────┐
      │          │───>│ session.rs   │
      │          │    │ save_current │
      │          │    │ load/list/   │
      │          │    │ delete/names │
      │          │    └──────────────┘
      │          │    ┌──────────────┐
      │          │───>│  stats.rs    │
      │          │    │ record usage │
      │          │    │ per-day JSON │
      │          │    │ at           │
      │          │    │ ~/.aictl/    │
      │          │    │ stats        │
      └────┬─────┘    └──────────────┘
           │
           ├──────────────────────────┐
           ▼                          ▼
      ┌──────────┐             ┌──────────┐
      │ llm*.rs  │             │ ui.rs    │
      │          │             │          │
      │ openai   │             │          │
      │ anthropic│             │ spinner  │
      │ gemini   │             │ confirm  │
      │ grok     │             │ render   │
      │ mistral  │             │          │
      │ deepseek │             │          │
      │ kimi     │             │          │
      │ zai      │             │          │
      │ ollama   │             │          │
      │ gguf     │             │          │
      │ mlx      │             │          │
      └──────────┘             └──────────┘
           │                          │
           ▼                          ▼
      LLM APIs               Terminal output
```

## On-Disk State (`~/.aictl/`)

All persistent state lives under `~/.aictl/`. Nothing is stored elsewhere, and no system environment variables or `.env` files are consulted for program parameters. The directory is created lazily — subdirectories are only materialized when first needed (e.g. `sessions/` on REPL startup, `agents/` on first agent save). The entire `~/.aictl/` tree is on the default blocked-paths list in `security.rs`, so tools cannot read or write inside it.

```
 ~/.aictl/
  ├── config              key=value settings file (provider, model, API keys, security & tool toggles)
  ├── hooks.json          user-defined lifecycle hooks (event → array of {matcher, command, timeout, enabled}); override path with AICTL_HOOKS_FILE
  ├── mcp.json            MCP server declarations ({mcpServers: {<name>: {command, args, env, enabled, timeout_secs}}}); override path with AICTL_MCP_CONFIG; gated behind AICTL_MCP_ENABLED=true
  ├── history             rustyline REPL input history (one entry per line)
  ├── stats               JSON array of per-day usage statistics (calls, tokens, estimated cost; written by stats.rs after every agent turn; consumed by /stats)
  ├── agents/             saved agent prompts — one plain-text file per agent
  │   ├── <name>          full system-prompt extension text; filename == agent name
  │   └── ...             (names validated: ASCII alphanumerics, `_`, `-`)
  ├── skills/             saved skills — one directory per skill, each with a SKILL.md
  │   ├── <name>/         directory name == skill name (ASCII alphanumerics, `_`, `-`)
  │   │   └── SKILL.md    YAML frontmatter (name, description) + markdown body
  │   └── ...             reserved names (built-in slash commands) are rejected at save time
  ├── models/             downloaded native local models, partitioned by backend
  │   ├── gguf/           GGUF files for the Local (llama.cpp) provider
  │   │   ├── <name>.gguf model file; filename stem is the local name shown in /model
  │   │   └── ...         (names validated: ASCII alphanumerics, `_`, `-`, `.`)
  │   └── mlx/            multi-file safetensors directories for the MLX provider (Apple Silicon)
  │       ├── <name>/     contains config.json, tokenizer.json, *.safetensors, etc.
  │       └── ...         (names default to `owner__repo`; validated as above)
  └── sessions/           persisted conversation histories
      ├── .names          tab-separated `uuid\tname` map (one entry per line, names unique, lowercase `[a-z0-9_]`)
      ├── <uuid-v4>       pretty-printed JSON: `{"id": "...", "messages": [{"role": ..., "content": ...}, ...]}`
      └── ...             (filename == session uuid; dotfiles are skipped by `list_sessions`)
```

### `~/.aictl/config`

Plain text, one `key=value` per line. Comments start with `#`; blank lines are ignored; a leading `export ` is stripped so the same file can be sourced by a shell if desired; values may be single- or double-quoted. Loaded at startup into a `static OnceLock<RwLock<HashMap<String, String>>>` by `config::load_config()` and read via `config::config_get(key)`. Writes go through `config::config_set(key, value)` (replaces in place or appends, creates the directory if missing) and deletions through `config::config_unset(key)`; both update the in-memory cache so subsequent reads see the change without restarting. API key reads are routed through `keys::get_secret` instead of `config_get`, which checks the system keyring first and only falls back to this file. CLI flags always override config values.

Recognized keys include:
- **Provider/model**: `AICTL_PROVIDER`, `AICTL_MODEL`
- **API keys**: `LLM_OPENAI_API_KEY`, `LLM_ANTHROPIC_API_KEY`, `LLM_GEMINI_API_KEY`, `LLM_GROK_API_KEY`, `LLM_MISTRAL_API_KEY`, `LLM_DEEPSEEK_API_KEY`, `LLM_KIMI_API_KEY`, `LLM_ZAI_API_KEY` (Ollama needs none), `FIRECRAWL_API_KEY` (for `search_web`). These can also live in the system keyring instead — see [API key storage](#api-key-storage-srckeysrs) below.
- **Behavior**: `AICTL_AUTO_COMPACT_THRESHOLD`, `AICTL_MEMORY` (`long-term`/`short-term`), `AICTL_INCOGNITO` (`true`/`false`), `AICTL_PROMPT_FILE` (default `AICTL.md`), `AICTL_PROMPT_FALLBACK` (default `true`; when enabled, a missing primary prompt file falls back to `CLAUDE.md` then `AGENTS.md`), `AICTL_TOOLS_ENABLED` (default `true`), `AICTL_LLM_TIMEOUT` (per-call LLM timeout in seconds; `0` disables; default `30`), `AICTL_SKILLS_DIR` (override the default `~/.aictl/skills/` location)
- **Security**: `AICTL_SECURITY_*` keys — blocked/allowed command lists, disabled tools, shell timeout, CWD jail toggles, prompt-injection guard (`AICTL_SECURITY_INJECTION_GUARD`, default `true`), audit log toggle (`AICTL_SECURITY_AUDIT_LOG`, default `true`), etc. (see `security.rs` and `audit.rs`)
- **Redaction**: `AICTL_SECURITY_REDACTION` (`off` / `redact` / `block`, default `off`), `AICTL_SECURITY_REDACTION_LOCAL` (default `false` — local providers bypass), `AICTL_REDACTION_DETECTORS` (subset of `api_key, aws, jwt, private_key, connection_string, credit_card, iban, email, phone, high_entropy`), `AICTL_REDACTION_EXTRA_PATTERNS` (semicolon-separated `NAME=REGEX` pairs → `[REDACTED:NAME]`), `AICTL_REDACTION_ALLOW` (semicolon-separated allowlist regexes), `AICTL_REDACTION_NER` (enable Layer-C NER, requires the `redaction-ner` cargo feature + a pulled model), `AICTL_REDACTION_NER_MODEL` (default `onnx-community/gliner_small-v2.1`). See `security/redaction.rs` and `security/redaction/ner.rs`.
- **Hooks**: `AICTL_HOOKS_FILE` (override the default `~/.aictl/hooks.json` path; used mainly by tests). The hook entries themselves live in the JSON file, not in `config`.
- **MCP**: `AICTL_MCP_ENABLED` (master switch, default `false`), `AICTL_MCP_CONFIG` (override the default `~/.aictl/mcp.json` path), `AICTL_MCP_TIMEOUT` (per-call RPC timeout, default 30s), `AICTL_MCP_STARTUP_TIMEOUT` (`initialize` handshake timeout, default 10s), `AICTL_MCP_DISABLED` (comma-separated server names to skip at init), `AICTL_MCP_DENY_SERVERS` (comma-separated server names blocked at the security gate). Server entries themselves live in `mcp.json`.

### API key storage (`crates/engine/src/keys.rs`)

API keys can live in two places: the plain-text `~/.aictl/config` file (the legacy default) or the OS-native keyring (macOS Keychain, Linux Secret Service). Lookups via `keys::get_secret(name)` check the keyring first and fall back to the config file, so users can mix the two during migration.

```
 ┌──────────────────────────────────────────────────────────────────┐
 │  keys::get_secret("LLM_OPENAI_API_KEY")                          │
 │       │                                                          │
 │       ▼                                                          │
 │  keyring::Entry::new("aictl", "LLM_OPENAI_API_KEY")              │
 │       │                                                          │
 │   ┌───┴────┐                                                     │
 │   ▼        ▼                                                     │
 │  Ok(v)    Err / NoEntry                                          │
 │   │        │                                                     │
 │   │        ▼                                                     │
 │   │   config_get("LLM_OPENAI_API_KEY")                           │
 │   │        │                                                     │
 │   ▼        ▼                                                     │
 │  return  Some(v) | None                                          │
 └──────────────────────────────────────────────────────────────────┘
```

`location(name)` returns a `KeyLocation::{None, Config, Keyring, Both}` for `/security` and the welcome banner counts. Migration commands operate on the canonical `KEY_NAMES` list (the eight LLM provider keys plus `FIRECRAWL_API_KEY`):

- `lock_key(name)` reads the value from the config file, writes it to the keyring, then calls `config_unset` to remove the plain-text copy. Exposed via the `/keys → lock keys` menu entry and the one-shot `--lock-keys` flag.
- `unlock_key(name)` reads the value from the keyring, writes it to the config file via `config_set`, then deletes the keyring entry. Exposed via `/keys → unlock keys` and `--unlock-keys`.
- `clear_key(name)` removes the entry from both stores. Exposed via `/keys → clear keys` (wrapped with a y/N confirmation) and `--clear-keys` (no confirmation; the explicit flag is treated as the user's consent).

The keyring backend is selected at compile time via Cargo features: `apple-native` on macOS, `sync-secret-service` on Linux. **Without explicit features the `keyring` v3 crate silently uses an in-memory mock store** that pretends writes succeed but never persists — `Cargo.toml` enables both platform backends to avoid this trap. `backend_available()` probes the active backend at runtime so headless Linux systems with no Secret Service daemon transparently fall back to plain-text storage and the welcome banner shows `keys: plain text` in yellow.

### `~/.aictl/audit/<session-id>`

JSONL audit log — one JSON object per line, appended on every tool invocation. The filename mirrors the corresponding session file under `~/.aictl/sessions/` so a reviewer can read both together. Each entry carries `timestamp` (UTC, ISO-8601 seconds precision), `tool`, `input` (truncated), and an `outcome` of `executed` (with `result_summary`), `denied_by_policy` (with `reason`), `denied_by_user`, `disabled`, or `duplicate`. Written by `crates/engine/src/audit.rs::log_tool`, called from `tools::execute_tool` for the policy / duplicate / disabled / executed outcomes and from `run::handle_tool_call` for the user-denial outcome. Skipped entirely in incognito mode and in single-shot (`--message`) runs where no session id exists. Toggled via `AICTL_SECURITY_AUDIT_LOG` in `~/.aictl/config` (default `true`); observability-only, so `--unrestricted` does not disable it.

The same file also carries redaction events when the redaction layer is active. These lines are written by `crates/engine/src/audit.rs::log_redaction` and carry `event: "redaction"`, `mode` (`redact` / `block`), `direction` (`outbound` / `inbound`), `source` (`system_prompt` / `user_message` / `assistant_message` / `tool_result`), and a `matches` array — one entry per detected span with `kind` (the placeholder label: `API_KEY`, `AWS_KEY`, `JWT`, `PRIVATE_KEY`, `CONNECTION_STRING`, `CREDIT_CARD`, `IBAN`, `EMAIL`, `PHONE`, `HIGH_ENTROPY`, `PERSON`, `LOCATION`, `ORGANIZATION`, or a user-defined name), byte `range`, `confidence`, and a scrubbed `snippet` (placeholder plus a few bytes of surrounding context — never the original secret). Same skip rules apply; toggle via the same `AICTL_SECURITY_AUDIT_LOG` key.

### `~/.aictl/history`

Plain text REPL input history managed by `rustyline`. Written on REPL exit; loaded at REPL startup. Not used by single-shot (`--message`) mode.

### `~/.aictl/agents/<name>`

Each file is an agent prompt — either pure prose or a markdown document that opens with a YAML frontmatter block (`name`, `description`, `source`, `category`). When an agent is loaded, the frontmatter is stripped and only the body is appended to the base system prompt under `# Agent: <name>`, so pulled catalogue agents don't leak their metadata to the LLM. Filenames are the agent names themselves (no extension), validated by `agents::is_valid_name` to contain only ASCII letters, digits, `_`, or `-`. Managed through `crates/engine/src/agents.rs`:
- `save_agent(name, prompt)` — creates `~/.aictl/agents/` if needed and writes the file verbatim
- `read_agent(name)` — raw file contents (frontmatter included) for edit round-trips
- `read_agent_meta(name)` — parsed `AgentMeta` (body + optional frontmatter fields)
- `delete_agent(name)` — remove the file
- `list_agents()` — enumerates regular files whose names pass validation, parses each one's frontmatter, and returns entries with `name`, `description`, `source`, `category` (sorted alphabetically; invalid filenames silently skipped)

Entries with `source: aictl-official` render an `[official]` badge in both the `/agent` REPL menu and `--list-agents` output. Users can edit or remove the marker freely — there is nothing enforcing it beyond the badge.

A global `Mutex<Option<(name, body)>>` in `agents.rs` holds at most one *loaded* agent for the current process; it is populated via `--agent <name>` at startup or via the `/agent` REPL menu, and cleared via `/agent → unload`.

The sibling `crates/engine/src/agents/remote.rs` module fetches the first-party catalogue on demand from the project repo under `.aictl/agents/*.md` via GitHub's trees API (+ raw.githubusercontent.com for bodies), with no API key required. Consumed by the REPL's `/agent → Browse official agents` entry and by `--pull-agent <name>` (add `--force` to overwrite). Pulls write a single `.md` file straight to `~/.aictl/agents/<name>` (stripping the extension); the catalogue itself is never bundled into the binary, so adding an agent to the repo is the full release.

### `~/.aictl/skills/<name>/SKILL.md`

Each skill lives in its own directory so future bundled resources (scripts, templates) can sit alongside the markdown without a layout migration. The file begins with YAML-ish frontmatter (`name`, `description`) followed by the markdown body — the procedure the LLM should follow when the skill is invoked. Managed entirely through `crates/engine/src/skills.rs`:

- `find(name)` — load one skill (directory name is authoritative; entries whose frontmatter `name` disagrees are skipped to avoid silent drift)
- `list()` — enumerate directories that contain a parseable `SKILL.md`, sorted alphabetically; each entry carries the name + one-line description for the menu
- `save(name, description, body)` — validates the name, rejects reserved slash-command names (e.g. `help`, `exit`, `agent`), creates the directory, writes `SKILL.md` with a regenerated frontmatter block
- `delete(name)` — recursive removal of the skill directory

The skills directory defaults to `~/.aictl/skills/` and can be redirected with `AICTL_SKILLS_DIR`. **Skills are never persisted into session history.** Invocation (via `/<skill-name>`, the menu's "invoke now", or `--skill <name>`) hands `Option<&Skill>` to `run_agent_turn`, which for that single call concatenates `# Skill: <name>\n\n<body>` onto `messages[0].content` (the base system prompt). This keeps the tool catalog intact: Anthropic and Gemini accept only a single top-level `system` field and overwrite on each `Role::System` they see, so injecting the skill as a separate system message would silently replace the tool rules. The persisted `Vec<Message>` the REPL saves to `~/.aictl/sessions/<uuid>` is untouched, so reloading a session later never replays a stale skill body.

### `~/.aictl/models/gguf/<name>.gguf`

Each file is a GGUF weight file for the native GGUF provider (`crates/engine/src/llm/gguf.rs`). The directory is created lazily on the first `--pull-gguf-model` or `/gguf → pull model`; by default it does not exist and no GGUF models are available. Downloads stream to `<name>.gguf.part` via `reqwest` with a `futures-util` async chunk loop and an `indicatif` progress bar, then atomically rename to `<name>.gguf` on success — an interrupted download never leaves a half-written model in place. Names are validated against `[A-Za-z0-9._-]+` and default to the GGUF file's stem (overridable at download time).

Management functions (all safe to compile without the `gguf` feature): `list_models()` scans `*.gguf`, `model_path(name)` resolves to the on-disk path, `remove_model(name)` deletes one file, `clear_models()` wipes the directory. `download_model(spec, override_name)` parses three spec forms — `hf:owner/repo/file.gguf`, `owner/repo:file.gguf`, and raw `https://…/file.gguf` — all routed through the same streaming download. `call_gguf()` is feature-gated: with `--features gguf` it loads the GGUF via `llama-cpp-2` on a `tokio::spawn_blocking` task, flattens messages into a ChatML-style prompt, and runs sampling up to 4096 new tokens; without the feature it returns an error telling the user to rebuild.

### `~/.aictl/models/mlx/<name>/`

Each subdirectory is a Hugging Face MLX model snapshot for the native MLX provider (`crates/engine/src/llm/mlx.rs`), containing at minimum `config.json`, `tokenizer.json`, `tokenizer_config.json`, and one or more `*.safetensors` files (with `model.safetensors.index.json` for sharded models). The parent `~/.aictl/models/mlx/` directory is created lazily on the first `--pull-mlx-model` or `/mlx → pull model`. Downloads walk the Hugging Face tree API, skip non-essential files (READMEs, images, alternate weight formats), and stream each file with a per-file `indicatif` progress bar into a `<name>.part/` staging directory that is renamed atomically on success.

Management functions (all safe to compile without the `mlx` feature, on every platform): `list_models()` enumerates subdirectories that contain a `config.json`, `model_path(name)` resolves to the directory, `remove_model(name)` recursively deletes one (with a defence-in-depth check that the canonical path is inside `models/mlx/`), `clear_models()` wipes every subdirectory, `model_size(name)` reports total on-disk bytes for the `/mlx` view. `download_model(spec, override_name)` parses two spec forms — `mlx:owner/repo` and `owner/repo` — both resolved against `huggingface.co/<owner>/<repo>`. `call_mlx()` is feature-gated: with `--features mlx` on `macos`+`aarch64` it builds a hand-written Llama-family transformer with `mlx-rs` primitives, hand-installs the quantized embedding (the `MaybeQuantized<Embedding>` derive doesn't expose its params), translates `q_proj.weight` → `q_proj.inner.weight` so safetensors keys match `QuantizedLinear`'s nested layout, renders the per-model jinja chat template via `minijinja` (ChatML fallback), and runs temperature-sampled generation with KV cache up to 4096 new tokens on a `tokio::spawn_blocking` task; without the feature or off Apple Silicon it returns a clear error telling the user how to enable native inference.

### `~/.aictl/plugins/<name>/`

Each subdirectory is a user-installed plugin tool, holding a `plugin.toml`
manifest plus an executable entrypoint (any language). Discovery,
manifest parsing, and subprocess execution live in `crates/engine/src/plugins.rs` —
see the **Plugins** section above for the full design. The directory is
not created by aictl; users drop plugins in by hand. Override the
discovery root with `AICTL_PLUGINS_DIR` (used by tests). The whole
subsystem is gated behind `AICTL_PLUGINS_ENABLED=true` (default off);
when off, no directory walk happens and the catalogue stays empty.
Per-plugin disable lives in config (`AICTL_PLUGINS_DISABLED=foo,bar`)
rather than in the manifest, so users don't have to edit a third-party
file to silence one.

### `~/.aictl/hooks.json`

User-defined lifecycle hooks loaded by `crates/engine/src/hooks.rs::init` at startup.
Top-level JSON object whose keys are event names and values are arrays
of hook entries `{ matcher, command, timeout, enabled }`. Underscore-
prefixed top-level keys (`_comment`, etc.) are silently skipped so the
file can carry inline notes despite JSON not supporting comments.
`AICTL_HOOKS_FILE` overrides the path (used by tests). The file is not
created by aictl; users drop it in by hand or via the `create-hook`
catalogue skill. A missing file leaves the hook table empty — all hook
fire sites become no-ops with zero overhead. Parse errors print a
diagnostic to stderr at startup but do not abort. See the Hooks section
above for the wire protocol and event semantics.

### `~/.aictl/sessions/`

Holds one JSON file per saved conversation plus a single `.names` index file. Managed entirely through `crates/engine/src/session.rs`.

**Session files** — filename is a UUID v4 generated from `/dev/urandom` (with a time-based fallback), produced by `session::generate_uuid()`. Content is pretty-printed JSON written by `save_messages`:

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "messages": [
    {"role": "system",    "content": "..."},
    {"role": "user",      "content": "..."},
    {"role": "assistant", "content": "..."}
  ]
}
```

The full `Vec<Message>` (including system prompt and any `<tool_result>` turns carried in `user` messages) is rewritten after every agent turn and after manual/auto compaction, so the file on disk always reflects the complete current conversation. In **incognito mode** (`--incognito` or `AICTL_INCOGNITO=true`), `save_current` short-circuits and no session file is ever created or updated. `list_sessions()` enumerates regular files in the directory (skipping dotfiles, so `.names` is excluded), sorted by mtime descending. `delete_session(id)` removes the file and its name mapping; `clear_all()` wipes every entry including `.names`.

**`.names` file** — a tab-separated index mapping `uuid\tname`, one entry per line, rewritten whole on every change by `write_names`. Names are normalized to lowercase and must match `[a-z0-9_]+`; they must be unique across sessions (enforced by `set_name`). Functions `name_for(id)` and `id_for_name(name)` perform lookups in either direction, and `resolve(key)` (used by `--session <id|name>`) first tries `key` as a uuid filename, then falls back to the name index.
