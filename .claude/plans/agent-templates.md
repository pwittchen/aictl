# Plan: Built-in Agent Templates

## Context

aictl ships with an `/agent` system for creating and loading persistent system-prompt extensions, but the built-in set is empty — every user starts from a blank slate. A small, curated collection of starter agents would give new users immediate value and illustrate the idiom (narrow persona + tool-surface guidance) so they can author their own.

These templates are **starting points**, not sacred. Users can load, view, and modify them like any other agent; they live in `~/.aictl/agents/<name>` alongside user-authored ones.

## Goals & Non-goals

**Goals**
- Ship a small, curated set of broadly useful agents that exercise different parts of the tool surface.
- Each template should be short (under ~30 lines), opinionated, and self-contained.
- Installation should be a first-run convenience, not a lock-in — users can delete or overwrite any template.

**Non-goals**
- No framework for third-party template distribution in v1.
- No per-provider or per-model tuning — templates are plain text prompts.
- No enforced tool subsets per template; tool access stays global.
- No single-turn procedures. Agents are session-long personas (`rust-expert`, `tech-writer`); single-turn procedures (`review`, `summarize-logs`, `inspect-cert`) live in `skill-templates.md`.

## Initial set

Chosen to cover distinct workflows and exercise different tool clusters.

### Dev & workflow

- **`researcher`** — answers questions with citations. Uses `search_web`, `fetch_url`, `extract_website`. Always includes source URLs.

- **`data-analyst`** — works over CSV/JSON with `csv_query`, `json_query`, `calculate`. Returns a table plus a one-line takeaway.

- **`shell-expert`** — explains or composes shell commands before running them. Prefers dry-runs; uses `system_info` / `list_processes` / `check_port` for diagnostics.

- **`image-specialist`** — analyzes or generates images. Uses `read_image` for vision (screenshots, diagrams, OCR-style transcription, alt-text) and `generate_image` for quick illustrations or mockups.

- **`bug-hunter`** — reproduces a bug, narrows it down with prints/logs, proposes a minimal fix. Leans on `run_code`, `exec_shell`, `search_files`.

- **`regex-expert`** — builds and explains regex patterns. Tests candidates against sample inputs via `run_code` before handing the final pattern back.

- **`prompt-engineer`** — refines and critiques LLM prompts. Asks for the target model and failure modes, then proposes a tightened prompt with a rationale for each change.

- **`software-architect`** — discusses high-level design and tradeoffs. Deliberately does *not* write code — outputs options, constraints, and a recommendation with the main risks called out.

- **`api-tester`** — pokes REST endpoints with `fetch_url` and filters responses through `json_query`. Uses `check_port` to sanity-check reachability before sending requests.

- **`sql-expert`** — writes and explains SQL across dialects (PostgreSQL, MySQL, SQLite). Tests candidate queries against a throwaway sqlite database via `run_code` before handing them back; flags `EXPLAIN` gotchas and obvious performance traps (missing indexes, N+1 shapes, accidental cross joins).

- **`git-archaeologist`** — uses `git log`, `git blame`, and `git diff` to explain why code exists, who wrote it, what issue or PR it fixed, and how it evolved. Useful when inheriting a codebase or untangling a tricky bug. Natural pair with `bug-hunter`.

### Ops

- **`sysadmin`** — machine diagnostics via `system_info`, `list_processes`, `check_port`. Uses `notify` on long-running completions.

- **`docker-operator`** — manages Docker containers, images, and Compose stacks via `exec_shell` (`docker ps`, `docker images`, `docker build`, `docker compose up/down/logs`). Reads and edits Dockerfiles and `docker-compose.yml`; uses `check_port` to verify published ports and `list_processes` to spot host-side conflicts. Prefers dry-runs and explains destructive commands (`rm`, `prune`, `down -v`) before running them.

- **`kubernetes-operator`** — manages Kubernetes resources via `kubectl` through `exec_shell`. Reads and edits YAML manifests, inspects pods/services/deployments, tails logs, and explains destructive commands (`delete`, `scale 0`, `drain`) before running them. Cluster-level analogue of `docker-operator`.

### Security

- **`security-auditor`** — greps for secrets, risky patterns, and unsafe APIs; runs dependency audits via `exec_shell`. Flags issues without auto-fixing.

- **`threat-modeler`** — applies STRIDE or attack-tree modeling to a described system. Pure-prompt, no tool use. Outputs threats grouped by category, mitigations, and residual risk; asks clarifying questions about trust boundaries and assets before modeling.

### Learning

- **`tutor`** — explains concepts at the requested level and produces small runnable examples via `run_code`.

### Knowledge work

- **`writer`** — drafts and tightens prose from a brief. Uses `read_document` for source material and `clipboard` to hand output back.

- **`editor`** — line-edits existing text for clarity and tone. Shows before/after; good for emails, posts, docs.

- **`copywriter`** — writes marketing and product copy (taglines, landing-page sections, ad copy, release notes) from a brief. Tunes voice and length to the channel; offers a few variants when asked.

### Data

- **`spreadsheet-analyst`** — reads Excel/ODS/CSV via `read_document` + `csv_query`, suggests formulas, cleans messy data, and pivots results into a quick summary table.

### Daily life

- **`travel-planner`** — drafts itineraries from a destination, dates, and budget. Uses `search_web` and `extract_website` for up-to-date info on venues, transit, and opening hours.

- **`budget-advisor`** — analyzes bank/card CSV exports via `csv_query`, categorizes spending, and surfaces patterns (subscription creep, recurring overruns) alongside realistic saving targets. Strong fit for the data-query tool cluster.

- **`sleep-coach`** — runs a brief sleep hygiene audit from a short questionnaire and suggests one or two concrete changes rather than a full overhaul. Flags patterns worth raising with a doctor rather than self-fixing.

- **`workout-coach`** — designs routines from available equipment, time budget, and goal (strength / cardio / mobility). Tunes difficulty and volume; tracks progression across sessions when you paste prior logs, and suggests swaps for exercises that don't fit your setup.

- **`kitesurfing-adivsor`** — plans kitesurfing sessions. Researches new spots (launch type, hazards, best wind direction, tide and local rules) via `search_web` + `extract_website`; pulls wind and weather forecasts from Windy / Windguru / local weather services via `fetch_url`; and recommends when to hit the water based on wind strength, direction, gust factor, tide, and precipitation. Suggests kite size from rider weight, skill level, board type, and forecast wind; flags marginal or unsafe conditions (offshore wind, thunderstorms, dangerous chop) rather than pushing you out.

### Thinking & habits

- **`decision-advisor`** — applies structured frameworks (pro/con, weighted-criteria, regret-minimization) to a concrete choice. Surfaces assumptions and what would change the answer.

- **`habit-coach`** — helps design a small, trackable habit with a cue, routine, reward, and check-in cadence. Keeps it modest on purpose — one habit at a time.

- **`critic`** — cold, objective, and blunt. Stress-tests ideas, plans, and arguments by identifying weak assumptions, missing evidence, logical gaps, and likely failure modes. Will say "this is wrong" or "this won't work" and explain why, with no flattery or hedging. Not rude for rudeness's sake — reasoning is always shown. Useful as a counterweight to the usual LLM agreeableness and as a natural pair with `brainstormer` for a generate-then-critique loop.

- **`first-principles-thinker`** — breaks a problem, belief, or design down to its fundamentals and rebuilds from the ground up. Distinct from `decision-advisor`'s framework-matching: instead of applying a shape, it asks what you actually know and what you're merely inheriting.

- **`mental-model-coach`** — picks a relevant mental model (second-order effects, Bayesian updating, Occam's razor, inversion, game theory, regret minimization) and applies it to the situation at hand. Explains why the model fits before using it.

### Creative & personal

- **`brainstormer`** — generates wide-then-narrow idea lists. Enforces "no self-critique until round two" so the first pass stays generative.

- **`journal-coach`** — asks reflective questions in a warm, non-judgmental tone. Pure-prompt, minimal tool use.

- **`psychologist`** — a supportive conversational persona that draws on psychology-informed techniques (active listening, validation, gentle CBT-style reframing, open-ended questions). Explicitly not a substitute for professional mental health care — includes a standing instruction to suggest reaching out to a qualified professional or crisis line when the user describes acute distress, self-harm, or harm to others. Pure-prompt, no tool use.

- **`storyteller`** — writes short fiction from a premise. Tunable length, genre, and POV; asks clarifying questions before drafting long pieces.

## Design sketch

**Storage**: templates live in the project git repo under `agents/*.md`, one file per agent. **Not** compiled into the binary — no `include_str!`, no bundled assets. The list is served from the GitHub repo at runtime so new templates can be added without a release.

**Frontmatter**: every bundled template starts with a YAML frontmatter block that marks it as app-provided and carries metadata. Example:

```markdown
---
name: code-reviewer
category: dev
source: aictl-official
description: Reviews staged/unstaged changes for correctness, security, style.
---

You are a code reviewer. …
```

The `source: aictl-official` key is the distinguishing marker — user-authored agents omit it (or set `source: user`), so the REPL and `--list-agents` can render a badge (e.g. `[official]`) next to bundled ones. `category` and `description` are used by the browse/list UIs.

**Browse & pull**: the `/agent` menu gains a **Browse agents** entry that fetches the directory listing from the GitHub repo at request time — no hardcoded manifest. Two fetch paths, in order:

1. GitHub REST: `GET https://api.github.com/repos/<owner>/<repo>/contents/agents?ref=master` returns a JSON array of files; for each `.md` entry the browser shows `name`, `category`, `description` parsed from frontmatter after a second fetch of `download_url` (or a single `git_trees` call with `?recursive=1` to avoid N+1 requests).
2. Fallback: raw `https://raw.githubusercontent.com/<owner>/<repo>/master/agents/<file>.md` for individual pulls.

The repo coordinates (`owner`, `repo`, `branch`) are constants in the binary — the *list* is dynamic, the *source location* is fixed. No API key is required for public-repo reads; rate limits (60/hr unauthenticated) are acceptable for this browse-then-pull flow.

**Pull flow**: selecting an agent in the browser downloads its `.md` to `~/.aictl/agents/<name>`. If a file with that name already exists, the REPL prompts `Agent <name> already exists. Overwrite? [y/N]` before writing. A single `--pull-agent <name>` CLI flag mirrors the menu for non-interactive use; `--pull-agent <name> --force` skips the prompt.

**Update indicator**: the browse UI tags each row with state:

- `[ ]` — not yet pulled
- `[✓]` — already on disk, frontmatter matches upstream
- `[↑]` — already on disk, upstream is newer (differing content or later commit SHA)

Pulling an `[↑]` row re-downloads and overwrites (still prompts unless the user opts into a session-wide "update all" action). Detection is content-hash based: compute SHA-256 of the local file and compare against the upstream blob SHA; fall back to byte-for-byte diff if needed.

**Discovery of installed agents**: the existing `/agent` view-all menu continues to list everything in `~/.aictl/agents/` exactly as today. The only UI change is the `[official]` badge on rows whose frontmatter has `source: aictl-official`. `--list-agents` adds the same badge.

**Categories**: agents carry a `category` field in frontmatter (e.g. `dev`, `ops`, `network`, `security`, `learning`, `knowledge-work`, `data-diagrams`, `daily-life`, `thinking-habits`, `creative`). Agents without a category fall into an `uncategorized` bucket. In the interactive `/agent` browse view (both local and remote) the user can pick **All** to see every agent in one flat list or drill into a specific category first. The category browser lists categories with a count next to each (e.g. `dev (10)`, `ops (2)`) and opens into the same row UI used today. The `--list-agents` CLI flag gains an optional `--category <name>` filter.

**Removal**: user deletes like any other agent via `/agent` or `rm ~/.aictl/agents/<name>`. Deletion works regardless of whether the agent was app-provided or user-authored — there's nothing sacred about official agents on disk.

## Open questions

- YAML frontmatter vs. a sidecar `<name>.meta` file vs. a single `~/.aictl/agents/.index`. Frontmatter is closest to the plain-text ethos but means the prompt file is no longer "just the prompt." Current leaning: frontmatter, because it round-trips cleanly when pulled from GitHub and doesn't require a parallel file per agent.
- GitHub API (metadata-rich but rate-limited) vs. raw CDN (unlimited but no directory listing). A hybrid — list via API, fetch via raw — keeps most of both, but what happens when the API rate limit is exhausted mid-browse? Show cached list from last successful fetch?
- Should the browser cache the remote listing to disk (e.g. `~/.aictl/agents/.remote-cache.json` with a short TTL) so repeat opens don't re-hit GitHub, or always fetch fresh? Fresh is simpler; cached is friendlier to flaky connections.
- Update check trigger: on-demand (user opens Browse) or periodic (background refresh on REPL startup)? On-demand is simpler and respects the no-surprise-network-calls principle.
- Is the `[↑]` upstream-newer detection worth the extra fetch per row, or should we just show `[✓]` and let the user re-pull if they want the latest? Defer until there's real feedback.
- Fixed category list vs. free-form? A fixed list keeps the browser tidy; free-form gives users more room. Compromise: define a fixed set for official agents, allow free-form on user agents, and group unknown values under "Other."

## Out of scope for v1

- Community template registry — only the official `aictl` repo's `agents/` dir is browsable. No arbitrary URL or third-party source support.
- Authenticated GitHub access (token-based higher rate limits).
- Signature verification on pulled agents — we trust the repo the same way we trust the binary.
- Background auto-updates of already-pulled agents.
- Template versioning beyond "pull overwrites" — no rollback, no history.
