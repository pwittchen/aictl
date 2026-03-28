# Architecture

## Module Structure

```
src/
 ├── main.rs          CLI args (clap), agent loop, single-shot & REPL modes
 ├── tools.rs          System prompt, XML tool-call parsing, tool execution
 ├── ui.rs             AgentUI trait, PlainUI & InteractiveUI implementations
 └── llm/
      ├── mod.rs       TokenUsage type, cost estimation (price_per_million)
      ├── openai.rs    OpenAI chat completions client
      └── anthropic.rs Anthropic messages client
```

## Startup Flow

```
 ┌──────────────────────────────────────────────────────────────────┐
 │  main()                                                          │
 │                                                                  │
 │  1. load_env_file()          read .env into process env vars     │
 │  2. Cli::parse()             parse --provider, --model, -M, ...  │
 │  3. resolve provider         flag > AICTL_PROVIDER env > error   │
 │  4. resolve model            flag > AICTL_MODEL env > error      │
 │  5. resolve api_key          OPENAI_API_KEY or ANTHROPIC_API_KEY │
 │  6. dispatch:                                                    │
 │     ├─ -M given ──> run_agent_single()  (PlainUI)                │
 │     └─ no -M ───> run_interactive()     (InteractiveUI + REPL)   │
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
 ┌──────────────────────────────────────────────────┐
 │  execute_tool(&ToolCall)                         │
 │                                                  │
 │  match tool_call.name:                           │
 │  ┌────────────────┬───────────────────────────┐  │
 │  │ Tool           │ Backend                   │  │
 │  ├────────────────┼───────────────────────────┤  │
 │  │ shell          │ sh -c (tokio::process)    │  │
 │  │ read_file      │ tokio::fs::read_to_string │  │
 │  │ write_file     │ tokio::fs::write          │  │
 │  │ edit_file      │ read + replacen + write   │  │
 │  │ list_directory │ tokio::fs::read_dir       │  │
 │  │ search_files   │ grep -rn (subprocess)     │  │
 │  │ glob           │ glob::glob                │  │
 │  │ web_search     │ Firecrawl API (reqwest)   │  │
 │  │ web_fetch      │ HTTP GET (reqwest)        │  │
 │  │ think          │ passthrough (no-op)       │  │
 │  └────────────────┴───────────────────────────┘  │
 │                                                  │
 │  All outputs truncated at 10,000 chars           │
 └──────────────────────────────────────────────────┘
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

## Data Flow (end to end)

```
 User ──> CLI args / REPL input
           │
           ▼
      ┌──────────┐    ┌──────────────┐
      │ main.rs  │───>│  tools.rs    │
      │          │    │              │
      │ agent    │    │ SYSTEM_PROMPT│
      │ loop     │    │ parse_tool() │
      │          │    │ execute_tool │
      └────┬─────┘    └──────────────┘
           │
           ├──────────────────────────┐
           ▼                          ▼
      ┌──────────┐             ┌──────────┐
      │ llm/     │             │ ui.rs    │
      │          │             │          │
      │ openai   │             │ spinner  │
      │ anthropic│             │ confirm  │
      │ tokens   │             │ render   │
      └──────────┘             └──────────┘
           │                          │
           ▼                          ▼
      LLM APIs               Terminal output
```
