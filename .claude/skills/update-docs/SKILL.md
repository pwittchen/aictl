---
name: update-docs
description: Update README.md, CLAUDE.md, and ARCH.md to match the current project state
allowed-tools: Bash, Read, Edit, Write, Glob, Grep
---

## Purpose

Synchronize all project documentation with the actual codebase. Read the source of truth (Rust source files, Cargo.toml, config) and update each doc file so it accurately reflects the current state — no stale references, no missing features, no wrong counts.

## Workflow

### 1. Gather current project state

Read these files to build an accurate picture:

- `Cargo.toml` — version, edition, dependencies and their versions
- `src/main.rs` — CLI args (clap struct fields), Provider enum variants, version constant, agent loop logic, modes
- `src/commands.rs` — COMMANDS constant array, CommandResult enum variants, slash command behavior
- `src/config.rs` — constants (MAX_ITERATIONS, MAX_MESSAGES, SPINNER_PHRASES count), SYSTEM_PROMPT (tool definitions), config keys
- `src/tools.rs` — tool names in `execute_tool` match arms, tool behavior
- `src/ui.rs` — AgentUI trait methods, PlainUI/InteractiveUI differences, constants (MAX_RESULT_LINES, MAX_ANSWER_WIDTH)
- `src/llm.rs` — MODELS constant (provider, name, config key), pricing in `price_per_million`, TokenUsage struct
- `src/llm_openai.rs` — API endpoint, request/response structs, max_tokens
- `src/llm_anthropic.rs` — API endpoint, headers, anthropic-version, max_tokens
- `.aictl.example` — config keys and descriptions

### 2. Build a checklist of facts

From the source code, extract these concrete values:

- **Version** number from `Cargo.toml` or `main.rs`
- **CLI parameters** — every clap field with short/long flags and descriptions
- **REPL commands** — the COMMANDS array, with what each does
- **Tools** — every match arm in `execute_tool`, with a one-line description
- **Models** — every entry in the MODELS constant, with provider and pricing
- **Config keys** — every key referenced by `config_get()` calls and in `.aictl.example`
- **Dependencies** — name and version from `Cargo.toml`
- **Module list** — every `.rs` file under `src/` with its responsibility
- **Constants** — MAX_ITERATIONS, MAX_MESSAGES, max_tokens per provider, output truncation limits

### 3. Update each documentation file

For each file, compare the checklist against what the doc currently says. Fix any discrepancies. Do NOT rewrite from scratch — preserve the existing structure and style, and make targeted edits.

#### README.md

- Version badge or mention (if present)
- CLI usage line and parameter table — must list every clap arg
- REPL commands list — must match COMMANDS array exactly
- Tools list — must match `execute_tool` arms exactly
- Models/pricing table — must match MODELS and `price_per_million`
- Config keys — must match `.aictl.example` and actual `config_get` calls
- Installation instructions — verify they still work

#### CLAUDE.md

- Build commands — verify they are correct
- Module list — every `src/*.rs` file with accurate one-line description
- Config description — correct keys, loading mechanism
- Flow description — matches actual code path
- Agent loop — correct iteration limit, message limit, confirmation behavior
- Tools list — matches `execute_tool` arms with accurate descriptions
- Provider details — correct endpoints, headers, max_tokens, message format
- Key dependencies — matches Cargo.toml

#### ARCH.md

- Module structure diagram — must list all current modules
- Startup flow — matches actual initialization in main.rs
- Agent loop diagram — correct limits and flow
- Tool dispatch — all tools listed
- LLM abstraction — correct providers and behavior
- UI layer — correct trait methods and implementations
- REPL command dispatch — all commands listed
- Data flow — accurate end-to-end path

### 4. Verify consistency

After editing, confirm:

- The same tool count appears in all three files
- The same command list appears in README.md and ARCH.md
- The same module list appears in CLAUDE.md and ARCH.md
- No doc references features or values that don't exist in source
- No source feature is missing from the docs

## Rules

- Preserve each file's existing formatting style and section structure.
- Do not add sections that don't already exist unless a major new feature has no coverage at all.
- Do not remove sections — if a section covers something that still exists, update it.
- Use exact values from source code (copy-paste counts, names, constants) — do not approximate.
- If a doc file doesn't exist yet, skip it — this skill updates existing docs only.
- Keep descriptions concise. Match the terseness level of the existing docs.
- Do not add emoji to any file.
- After all edits, do a final read of each changed file to confirm correctness.
