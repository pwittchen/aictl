---
name: update-docs
description: Update README.md, CLAUDE.md, ARCH.md, and the website (website/index.html, website/guides.html) to match the current project state
allowed-tools: Bash, Read, Edit, Write, Glob, Grep
---

## Purpose

Synchronize all project documentation with the actual codebase. Read the source of truth (Rust source files, Cargo.toml, config) and update each doc file so it accurately reflects the current state — no stale references, no missing features, no wrong counts.

Scope covers both repo-level docs (`README.md`, `CLAUDE.md`, `ARCH.md`) and the public website under `website/` (`index.html`, `guides.html`). The Rust source is authoritative; website copy must stay aligned with it.

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
- `.aictl/config` — config keys and descriptions

### 2. Build a checklist of facts

From the source code, extract these concrete values:

- **Version** number from `Cargo.toml` or `main.rs`
- **CLI parameters** — every clap field with short/long flags and descriptions
- **REPL commands** — the COMMANDS array, with what each does
- **Tools** — every match arm in `execute_tool`, with a one-line description
- **Models** — every entry in the MODELS constant, with provider and pricing
- **Config keys** — every key referenced by `config_get()` calls and in `.aictl/config`
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
- Config keys — must match `.aictl/config` and actual `config_get` calls
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

#### website/index.html

The public landing page. Marketing copy, but the concrete numbers and names must still match the source. Check:

- `<meta name="description">` and `<meta property="og:description">` — provider count, model count, mention of Ollama / GGUF / MLX
- Version tag in the hero (e.g. `v0.24.2 · Rust · single binary · open source`) — must match `Cargo.toml`
- Hero headline and subhead — model count, provider count
- Features section — tool count ("17 built-in tools"), iteration limit ("Up to 20 tool iterations"), security feature list (must match `src/security.rs` actual mechanisms)
- Providers section — title ("Eleven providers, one interface") and the `<ul class="providers">` list (must match the Provider enum + local backends)
- Tools section — the `<ul>` entries under `<div class="tools">` blocks must be a faithful subset of `execute_tool` arms with accurate one-liners
- Comparison/differentiators copy — provider count, local-inference claims, security feature names
- Install section — commands must still work
- Nav anchors — every `href="#..."` must resolve to an `id` that still exists

#### website/guides.html

The long-form user guide. Same source-of-truth rules as index.html, plus more detail. Check:

- `<meta name="description">` — topic list matches sections on the page
- Nav links across the top — anchors point to real ids
- Table-of-contents list — every bullet points to a real section id
- Install section — command matches `install.sh`
- Configuration section — config keys (`AICTL_PROVIDER`, `AICTL_MODEL`, `AICTL_TOOLS_ENABLED`, `AICTL_MEMORY`, `AICTL_INCOGNITO`, `AICTL_LLM_TIMEOUT`, `AICTL_SECURITY_*`, `AICTL_PROMPT_FILE`, etc.) — must match `src/config.rs` and `src/security.rs`
- API keys section — one entry per remote provider, key name matches `KEY_NAMES` in `src/keys.rs`
- Providers & models section — one card per provider, model families and vision/image-gen claims match `src/llm.rs` MODELS and the image-capability matrix in CLAUDE.md
- REPL commands section — must list every entry in the `COMMANDS` array in `src/commands.rs` with accurate descriptions
- Agents section — describes `/agent` menu, `--agent`, `--list-agents`, `~/.aictl/agents/` path
- Sessions section — describes UUID/named sessions, `--session`, `--list-sessions`, `--clear-sessions`, `--incognito`, `AICTL_INCOGNITO`
- Tools section — entry per `execute_tool` arm with accurate description
- Local models section — GGUF (`/gguf`, `--pull-gguf-model`, etc.) and MLX (`/mlx`, `--pull-mlx-model`, etc.) flows match `src/llm_gguf.rs` and `src/llm_mlx.rs`; feature-flag wording matches `Cargo.toml` `[features]`
- Security section — matches `src/security.rs` policy (CWD jail, blocked paths, env scrub, timeout, injection guard, `--unrestricted`)

Rules specific to the website:

- Keep the existing HTML structure, class names, and `id`s. Don't restructure the DOM for stylistic reasons — `style.css` depends on it and [`website/DESIGN.md`](../../../website/DESIGN.md) is load-bearing.
- Preserve HTML entities as-is (`&mdash;`, `&amp;`). Don't convert to raw Unicode inside entities that are already encoded.
- Don't introduce new `id`s without also updating the nav and ToC that link to them; don't remove an `id` that's linked from elsewhere.
- No emoji, no new frameworks, no new external assets.
- Do not edit anything under `website/dist/` — it's a build output. `bun run build` regenerates it from the source files.

### 4. Verify consistency

After editing, confirm:

- The same tool count appears in README.md, CLAUDE.md, ARCH.md, `website/index.html`, and `website/guides.html`
- The same provider count and list appear in README.md, CLAUDE.md, `website/index.html`, and `website/guides.html`
- The same version string appears in `Cargo.toml`, `src/main.rs` (if hardcoded), README.md (if referenced), and the hero tag in `website/index.html`
- The same command list appears in README.md, ARCH.md, and `website/guides.html` REPL section
- The same module list appears in CLAUDE.md and ARCH.md
- No doc references features or values that don't exist in source
- No source feature is missing from the docs
- Every website anchor (`href="#..."` and cross-page `href="index.html#..."`) resolves to an existing `id`

## Rules

- Preserve each file's existing formatting style and section structure.
- Do not add sections that don't already exist unless a major new feature has no coverage at all.
- Do not remove sections — if a section covers something that still exists, update it.
- Use exact values from source code (copy-paste counts, names, constants) — do not approximate.
- If a doc file doesn't exist yet, skip it — this skill updates existing docs only.
- Keep descriptions concise. Match the terseness level of the existing docs.
- Do not add emoji to any file.
- After all edits, do a final read of each changed file to confirm correctness.
