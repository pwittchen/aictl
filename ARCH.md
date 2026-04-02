# Architecture

## Module Structure

```
src/
 ├── main.rs            CLI args (clap), agent loop, single-shot & REPL modes
 ├── commands.rs         REPL slash commands (/clear, /compact, /context, /copy, /exit, /help, /info, /mode, /model, /security, /tools, /update)
 ├── config.rs           Config file loading (~/.aictl), constants (system prompt, spinner phrases, agent loop limits)
 ├── security.rs         SecurityPolicy, shell/path/env validation, CWD jail, timeout, output sanitization
 ├── tools.rs            XML tool-call parsing, tool execution dispatch (security gate + output sanitization)
 ├── ui.rs               AgentUI trait, PlainUI & InteractiveUI implementations
 ├── llm.rs              TokenUsage type, cost estimation (price_per_million), model list, context limits
 ├── llm_openai.rs       OpenAI chat completions client
 └── llm_anthropic.rs    Anthropic messages client
```

## Startup Flow

```
 ┌──────────────────────────────────────────────────────────────────┐
 │  main()                                                          │
 │                                                                  │
 │  1. load_config()            read ~/.aictl into OnceLock HashMap  │
 │  2. Cli::parse()             parse --provider, --model, -m, ...  │
 │  2b. security::init()        load SecurityPolicy into OnceLock   │
 │  3. resolve provider         flag > AICTL_PROVIDER config > error│
 │  4. resolve model            flag > AICTL_MODEL config > error   │
 │  5. resolve api_key          OPENAI_API_KEY or ANTHROPIC_API_KEY │
 │  6. dispatch:                                                    │
 │     ├─ -m given ──> run_agent_single()  (PlainUI)                │
 │     └─ no -m ───> run_interactive()     (InteractiveUI + REPL)   │
 └──────────────────────────────────────────────────────────────────┘
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
               ┌─────────┴─────────┐
               ▼                   ▼
 ┌──────────────────┐   ┌───────────────────┐
 │  call_openai()   │   │ call_anthropic()  │
 │                  │   │                   │
 │  System msg      │   │  System msg ──>   │
 │  inline in       │   │  top-level        │
 │  messages[]      │   │  "system" field   │
 │                  │   │                   │
 │  POST /v1/chat/  │   │  POST /v1/        │
 │  completions     │   │  messages         │
 └────────┬─────────┘   └─────────┬─────────┘
          │                       │
          └───────────┬───────────┘
                      ▼
           ┌──────────────────┐
           │ (String,         │
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

 Also: /context (Context), /info (Info), /security (Security), /mode (Mode), /model (Model), /tools (Continue), /update (Update)

 CommandResult enum:
   Exit        → break REPL loop
   Clear       → reset messages & last_answer, continue
   Compact     → summarize conversation via LLM, continue
   Context     → show token/message usage, continue
   Info        → show provider/model/version info, continue
   Security    → show current security policy, continue
   Update      → run update, restart if updated, continue
   Model       → select new model/provider, persist to ~/.aictl, continue
   Mode        → switch auto/human-in-the-loop mode, continue
   Continue    → command handled, continue
   NotACommand → pass input to agent loop
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
      └────┬─────┘    └──────────────┘
           │
           ├──────────────────────────┐
           ▼                          ▼
      ┌──────────┐             ┌──────────┐
      │ llm*.rs  │             │ ui.rs    │
      │          │             │          │
      │ openai   │             │ spinner  │
      │ anthropic│             │ confirm  │
      │ tokens   │             │ render   │
      └──────────┘             └──────────┘
           │                          │
           ▼                          ▼
      LLM APIs               Terminal output
```
