---
name: update-docs
description: Update README.md, CLAUDE.md, the docs/ tree (ARCH.md, INSTALL.md, USAGE.md, CONFIG.md, PROVIDERS.md, TOOLS.md, EXTENSIONS.md, CODING_AGENT.md, SERVER.md), and the website (website/index.html, website/terminal.html, website/server.html, website/desktop.html) to match the current project state
allowed-tools: Bash, Read, Edit, Write, Glob, Grep
---

## Purpose

Synchronize all project documentation with the actual codebase. Read the source of truth (Rust source files, Cargo.toml, config) and update each doc file so it accurately reflects the current state — no stale references, no missing features, no wrong counts.

Scope covers both repo-level docs (`README.md` and `CLAUDE.md` at the root; `docs/ARCH.md`, `docs/INSTALL.md`, `docs/USAGE.md`, `docs/CONFIG.md`, `docs/PROVIDERS.md`, `docs/TOOLS.md`, `docs/EXTENSIONS.md`, `docs/CODING_AGENT.md`, `docs/SERVER.md`, `docs/LLM_PRICING.md`, `docs/COMMERCIAL_PRICING.md`, `docs/DESIGN.md`) and the public website under `website/` (`index.html`, `terminal.html`, `server.html`, `desktop.html`). The Rust source is authoritative; doc and website copy must stay aligned with it.

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

#### README.md (root)

`README.md` is now a thin landing page (~120 lines). It should still cover:

- Tagline / what is aictl
- Install one-liner
- Quick-start examples
- The Documentation index table that points to the docs under `docs/`
- Brief mention of `aictl-server`
- Support & consulting
- License

Most of the prose that used to live in README has been split into the files under `docs/` (see the sub-sections below).

#### docs/INSTALL.md

- One-liner installer command and supported platforms (must match `install.sh` and the release matrix)
- Build-from-source steps (`cargo install --path crates/aictl-cli`)
- Optional cargo features (`gguf`, `mlx`, `redaction-ner`) and platform/toolchain requirements
- Docker build and run examples (must match `docker/cli.Dockerfile`)
- Desktop app build steps (must match `crates/aictl-desktop/`)
- Uninstall procedure

#### docs/USAGE.md

- Full clap usage line and parameter table — must list every clap arg in `crates/aictl-cli/src/main.rs`
- REPL slash-command table — must match the `COMMANDS` array in `crates/aictl-cli/src/commands.rs`
- Sessions explanation
- Examples block (single-shot, `--auto`, `--quiet`, `--format json`, etc.)

#### docs/CONFIG.md

- Basic config keys and descriptions (must match `crates/aictl-core/src/config.rs`)
- API key names and provider console URLs (must match `KEY_NAMES` in `crates/aictl-core/src/keys.rs`)
- Secure key storage explanation
- Security configuration keys (`AICTL_SECURITY*`, `AICTL_REDACTION*`, plugin / hook / MCP master switches) — must match `crates/aictl-core/src/security.rs`

#### docs/PROVIDERS.md

- One section per provider with the per-token pricing table — must match `MODELS` and `price_per_million()` in `crates/aictl-core/src/llm.rs`
- Local-provider sections (Ollama / GGUF / MLX) with model-management commands and feature-flag notes
- Cost-estimate paragraph linking to `docs/LLM_PRICING.md`

#### docs/TOOLS.md

- Built-in tools table — must match `execute_tool` arms (`crates/aictl-core/src/tools.rs`)
- Image capabilities matrix — must match `IMAGE_GEN_MODELS` + `is_vision_capable` in `crates/aictl-core/src/llm.rs`
- XML tool format example
- Security gate description

#### docs/EXTENSIONS.md

- Agents, skills, plugins, hooks, MCP servers — one section each
- Config keys / file paths for each subsystem (must match the relevant `crates/aictl-core/src/{agents,skills,plugins,hooks,mcp}.rs` modules)

#### docs/CODING_AGENT.md

- Coding-agent mode toggles (CLI flag, slash command, config key, desktop)
- Five-phase workflow description
- Tuning knobs (`AICTL_CODING_*`) — must match `crates/aictl-core/src/coding.rs`
- Repo context block + Review hook description

#### docs/SERVER.md

- Full `aictl-server` reference (endpoints, configuration, security, deployment) — see `crates/aictl-server/src/` for source of truth

#### CLAUDE.md (root)

- Build commands — verify they are correct
- Workspace layout (`crates/aictl-{cli,core,server,desktop}`) — every crate with accurate one-line description
- Module map — every module file with accurate one-line description
- Key non-obvious behaviors
- Conventions

#### docs/ARCH.md

- Module structure diagram — must list all current modules
- Startup flow — matches actual initialization in `crates/aictl-cli/src/main.rs`
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

#### website/terminal.html

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

- Keep the existing HTML structure, class names, and `id`s. Don't restructure the DOM for stylistic reasons — `style.css` depends on it and [`docs/DESIGN.md`](../../../docs/DESIGN.md) is load-bearing.
- Preserve HTML entities as-is (`&mdash;`, `&amp;`). Don't convert to raw Unicode inside entities that are already encoded.
- Don't introduce new `id`s without also updating the nav and ToC that link to them; don't remove an `id` that's linked from elsewhere.
- No emoji, no new frameworks, no new external assets.
- Do not edit anything under `website/dist/` — it's a build output. `bun run build` regenerates it from the source files.

### 4. Verify consistency

After editing, confirm:

- The same tool count appears in `docs/TOOLS.md`, `CLAUDE.md`, `docs/ARCH.md`, `website/index.html`, and `website/terminal.html`
- The same provider count and list appear in `docs/PROVIDERS.md`, `CLAUDE.md`, `website/index.html`, and `website/terminal.html`
- The same version string appears in `Cargo.toml`, `crates/aictl-cli/src/main.rs` (if hardcoded), and the hero tag in `website/index.html`
- The same command list appears in `docs/USAGE.md`, `docs/ARCH.md`, and `website/terminal.html` REPL section
- The same module list appears in `CLAUDE.md` and `docs/ARCH.md`
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
