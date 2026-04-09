# Architecture

## Module Structure

```
src/
 ├── main.rs            CLI args (clap), agent loop, single-shot & REPL modes, session init
 ├── agents.rs           Agent prompt management (~/.aictl/agents/), loaded-agent state, CRUD, name validation
 ├── commands.rs         REPL slash commands (/agent, /behavior, /clear, /compact, /context, /copy, /exit, /help, /info, /issues, /model, /security, /session, /thinking, /tools, /update)
 ├── config.rs           Config file loading (~/.aictl/config), constants (system prompt, spinner phrases, agent loop limits), project prompt file loading
 ├── security.rs         SecurityPolicy, shell/path/env validation, CWD jail, timeout, output sanitization
 ├── session.rs          Session persistence (~/.aictl/sessions/), UUID v4 generation, JSON save/load, names file, incognito toggle
 ├── tools.rs            XML tool-call parsing, tool execution dispatch (security gate + output sanitization)
 ├── ui.rs               AgentUI trait, PlainUI & InteractiveUI implementations
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
 │  1. load_config()            read ~/.aictl/config into OnceLock HashMap  │
 │  2. Cli::parse()             parse --provider, --model, -m, ...          │
 │  2b. security::init()        load SecurityPolicy into OnceLock           │
 │  2c. --list-sessions /       non-interactive session helpers, exit       │
 │      --clear-sessions                                                    │
 │  2c'. --list-agents          non-interactive agent listing, exit         │
 │  2d. --config                run_config_wizard() and exit                │
 │  3. resolve provider         flag > AICTL_PROVIDER config > error        │
 │  4. resolve model            flag > AICTL_MODEL config > error           │
 │  5. resolve api_key          LLM_{OPENAI,ANTHROPIC,GEMINI,GROK,          │
 │                              MISTRAL,DEEPSEEK,KIMI,ZAI}_API_KEY          │
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
 │  └─────────────────────┴───────────────────────────┘      │
 │                                                           │
 │                                                           │
 │  3. sanitize_output() ── escape <tool> tags in results    │
 │  All outputs truncated at 10,000 chars                    │
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

 Also: /agent (Agent), /behavior (Behavior), /thinking (Thinking), /context (Context), /info (Info), /issues (Issues), /security (Security), /session (Session), /model (Model), /tools (Continue), /update (Update)

 CommandResult enum:
   Exit        → break REPL loop
   Clear       → reset messages & last_answer, continue
   Compact     → summarize conversation via LLM, save session, continue
   Agent       → open agent menu (create manually / create with AI / view all / unload);
                 loading/unloading rebuilds system prompt; continue
   Context     → show token/message usage, continue
   Info        → show provider/model/version/agent info, continue
   Security    → show current security policy, continue
   Session     → open session menu (current info / set name / view saved / clear all);
                 disabled in incognito mode; continue
   Issues      → fetch and display known issues, continue
   Update      → run update, restart if updated, continue
   Model       → select new model/provider, persist to ~/.aictl/config, continue
   Behavior    → switch auto/human-in-the-loop behavior, continue
   Thinking    → switch thinking mode (smart/fast), persist to ~/.aictl/config, continue
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
