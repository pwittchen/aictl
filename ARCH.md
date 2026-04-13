# Architecture

## Module Structure

```
src/
 ├── main.rs            CLI args (clap), agent loop, single-shot & REPL modes, session init
 ├── agents.rs           Agent prompt management (~/.aictl/agents/), loaded-agent state, CRUD, name validation
 ├── commands.rs         REPL slash commands (/agent, /behavior, /clear, /clear-keys, /compact, /config, /context, /copy, /exit, /help, /info, /issues, /lock-keys, /memory, /model, /security, /session, /tools, /unlock-keys, /update, /version)
 ├── config.rs           Config file loading (~/.aictl/config) into RwLock-backed cache, constants (system prompt, spinner phrases, agent loop limits), project prompt file loading
 ├── keys.rs             Secure API key storage. System keyring (Keychain / Secret Service) with transparent plain-text fallback. lock_key/unlock_key/clear_key migration primitives.
 ├── security.rs         SecurityPolicy, shell/path/env validation, CWD jail, timeout, output sanitization
 ├── session.rs          Session persistence (~/.aictl/sessions/), UUID v4 generation, JSON save/load, names file, incognito toggle
 ├── tools.rs            XML tool-call parsing, tool execution dispatch (security gate + output sanitization)
 ├── ui.rs               AgentUI trait, PlainUI & InteractiveUI implementations (welcome banner shows key storage backend)
 ├── llm.rs              TokenUsage type, cost estimation (price_per_million), model list, context limits
 ├── llm_openai.rs       OpenAI chat completions client
 ├── llm_anthropic.rs    Anthropic messages client
 ├── llm_gemini.rs       Google Gemini generateContent client
 ├── llm_grok.rs         xAI Grok chat completions client
 ├── llm_mistral.rs      Mistral chat completions client
 ├── llm_deepseek.rs     DeepSeek chat completions client
 ├── llm_kimi.rs         Kimi (Moonshot AI) chat completions client
 ├── llm_zai.rs          Z.ai chat completions client
 └── llm_ollama.rs       Ollama local model client (dynamic model discovery via /api/tags)
```

## Startup Flow

```
 ┌──────────────────────────────────────────────────────────────────────────┐
 │  main()                                                                  │
 │                                                                          │
 │  1. load_config()            read ~/.aictl/config into RwLock<HashMap>   │
 │  2. Cli::parse()             parse --provider, --model, -m, ...          │
 │  2b. security::init()        load SecurityPolicy into OnceLock           │
 │  2c. --list-sessions /       non-interactive session helpers, exit       │
 │      --clear-sessions                                                    │
 │  2c'. --list-agents          non-interactive agent listing, exit         │
 │  2d. --config                run_config_wizard() and exit                │
 │  3. resolve provider         flag > AICTL_PROVIDER config > error        │
 │  4. resolve model            flag > AICTL_MODEL config > error           │
 │  5. resolve api_key          keys::get_secret(LLM_*_API_KEY)             │
 │                              keyring first, plain-text config fallback   │
 │                              (Ollama: no key needed)                     │
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
 │  security::detect_prompt_injection() ── block on match  │
 │  (guard; gated by AICTL_SECURITY_INJECTION_GUARD)       │
 │                                                         │
 │  Append user message to Vec<Message>                    │
 │                                                         │
 │  for _ in 0..MAX_ITERATIONS (20) {                      │
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
 │   return   ┌────────────────────┐                       │
 │   answer   │  Confirm or --auto │                       │
 │            └────────┬───────────┘                       │
 │                 ┌───┴───┐                               │
 │              denied   approved                          │
 │                 │       │                               │
 │                 ▼       ▼                               │
 │          push deny   execute_tool()                     │
 │          message     push <tool_result> to messages     │
 │                 │       │                               │
 │                 └───┬───┘                               │
 │                     │                                   │
 │                     ▼                                   │
 │              next iteration ─────────────────────────>  │
 │  }                                                      │
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
 │  │ read_document       │ pdf-extract / zip+XML     │      │
 │  └─────────────────────┴───────────────────────────┘      │
 │                                                           │
 │                                                           │
 │  3. sanitize_output() ── escape <tool> tags in results    │
 │  All outputs truncated at 10,000 chars                    │
 │                                                           │
 │  Notes:                                                   │
 │  - read_image attaches ImageData to Message; providers    │
 │    encode it in their native vision format                │
 │  - generate_image auto-selects provider by available key: │
 │    active provider first, then OpenAI > Gemini > Grok     │
 │  - read_document dispatches by extension: .pdf via        │
 │    pdf-extract, .docx via zip + XML-to-markdown parser    │
 └───────────────────────────────────────────────────────────┘
```

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

 Also: /agent (Agent), /behavior (Behavior), /memory (Memory), /context (Context), /info (Info), /issues (Issues), /security (Security), /session (Session), /model (Model), /tools (Continue), /lock-keys (LockKeys), /unlock-keys (UnlockKeys), /clear-keys (ClearKeys), /config (Config), /update (Update), /version (Version)

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
   Issues      → fetch and display known issues, continue
   LockKeys    → migrate plain-text API keys from config into the system keyring, continue
   UnlockKeys  → migrate API keys from the system keyring back into config, continue
   ClearKeys   → remove API keys from both config and keyring (with confirmation), continue
   Update      → run update, restart if updated, continue
   Version     → check current version against latest available, continue
   Config      → re-run interactive configuration wizard, continue
   Model       → select new model/provider, persist to ~/.aictl/config, continue
   Behavior    → switch auto/human-in-the-loop behavior, continue
   Memory      → switch memory mode (long-term/short-term), persist to ~/.aictl/config, continue
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
      │          │───>│ session.rs   │
      │          │    │ save_current │
      │          │    │ load/list/   │
      │          │    │ delete/names │
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
  ├── history             rustyline REPL input history (one entry per line)
  ├── agents/             saved agent prompts — one plain-text file per agent
  │   ├── <name>          full system-prompt extension text; filename == agent name
  │   └── ...             (names validated: ASCII alphanumerics, `_`, `-`)
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
- **Behavior**: `AICTL_AUTO_COMPACT_THRESHOLD`, `AICTL_MEMORY` (`long-term`/`short-term`), `AICTL_INCOGNITO` (`true`/`false`), `AICTL_PROMPT_FILE` (default `AICTL.md`), `AICTL_TOOLS_ENABLED` (default `true`)
- **Security**: `AICTL_SECURITY_*` keys — blocked/allowed command lists, disabled tools, shell timeout, CWD jail toggles, prompt-injection guard (`AICTL_SECURITY_INJECTION_GUARD`, default `true`), etc. (see `security.rs`)

### API key storage (`src/keys.rs`)

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

- `lock_key(name)` reads the value from the config file, writes it to the keyring, then calls `config_unset` to remove the plain-text copy.
- `unlock_key(name)` reads the value from the keyring, writes it to the config file via `config_set`, then deletes the keyring entry.
- `clear_key(name)` removes the entry from both stores; the slash command wraps this with a y/N confirmation.

The keyring backend is selected at compile time via Cargo features: `apple-native` on macOS, `sync-secret-service` on Linux. **Without explicit features the `keyring` v3 crate silently uses an in-memory mock store** that pretends writes succeed but never persists — `Cargo.toml` enables both platform backends to avoid this trap. `backend_available()` probes the active backend at runtime so headless Linux systems with no Secret Service daemon transparently fall back to plain-text storage and the welcome banner shows `keys: plain text` in yellow.

### `~/.aictl/history`

Plain text REPL input history managed by `rustyline`. Written on REPL exit; loaded at REPL startup. Not used by single-shot (`-m`) mode.

### `~/.aictl/agents/<name>`

Each file is a plain-text agent prompt — the body that gets appended to the base system prompt under `# Agent: <name>` when the agent is loaded. Filenames are the agent names themselves (no extension), validated by `agents::is_valid_name` to contain only ASCII letters, digits, `_`, or `-`. Managed entirely through `src/agents.rs`:
- `save_agent(name, prompt)` — creates `~/.aictl/agents/` if needed and writes the file
- `read_agent(name)` / `delete_agent(name)` — load/remove a single agent
- `list_agents()` — enumerates regular files whose names pass validation, sorted alphabetically (invalid filenames are silently skipped)

A global `Mutex<Option<(name, prompt)>>` in `agents.rs` holds at most one *loaded* agent for the current process; it is populated via `--agent <name>` / `-A <name>` at startup or via the `/agent` REPL menu, and cleared via `/agent → unload`.

### `~/.aictl/sessions/`

Holds one JSON file per saved conversation plus a single `.names` index file. Managed entirely through `src/session.rs`.

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
