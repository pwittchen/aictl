# Plan: Built-in Agent Templates

## Context

aictl's `/agent` system lets users create and load persistent system-prompt extensions, but the built-in set is empty — every user starts from a blank slate. The remote catalogue (see `agents-remote-catalogue.md`) ships the browse-and-pull machinery: `/agent` can list official agents from the project's GitHub repo and write them to `~/.aictl/agents/<name>` on demand.

That plan covers *how*. This one covers *what*: the initial curated set of official agents that lands in `agents/` in the repo so the catalogue has real content worth browsing on day one.

These templates are **starting points**, not sacred. Users can pull, view, modify, re-pull, and delete them like any other agent. They're distinguishable from user-authored agents only by the `source: aictl-official` frontmatter key.

## Goals & Non-goals

**Goals**
- Ship a curated set of broadly useful official agents that exercise different parts of the tool surface.
- Each agent should be short (under ~30 lines), opinionated, and self-contained.
- Favor agents that are genuinely session-shaped: a persona you want to keep loaded for a whole conversation.
- Keep agent names noun-phrase role names (`bug-hunter`, `software-architect`, `journal-coach`) so the loaded-persona UX reads naturally.

**Non-goals**
- No new delivery mechanism. `agents-remote-catalogue.md` owns the browse/pull flow; this plan just seeds `agents/` in the repo.
- No per-provider or per-model tuning — templates are plain text prompts.
- No enforced tool subsets per template; tool access stays global.
- Nothing one-shot or procedure-shaped. Single-turn procedures (`review`, `summarize-logs`, `inspect-cert`) live in `skill-templates.md`.

## Initial set

Chosen to cover distinct workflows and exercise different tool clusters. Categories mirror the set used in `skill-templates.md` so the browse UI feels consistent across both content types.

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

**Storage**: each agent lives in the project git repo under `agents/<name>.md`, one file per agent. Same layout as `~/.aictl/agents/` on disk so pulls are a straight copy.

**Frontmatter**: every bundled agent carries the official marker and category:

```markdown
---
name: bug-hunter
description: Reproduces a bug, narrows it down, proposes a minimal fix.
source: aictl-official
category: dev
---

You are a bug hunter...
```

The `source: aictl-official` key is what the REPL uses to render an `[official]` badge next to pulled agents. `category` drives the browse-UI drill-down. Both fields are already specified in `agents-remote-catalogue.md` — this plan just populates them.

**Delivery**: nothing new to build. The browse-and-pull machinery in `agents-remote-catalogue.md` picks these files up automatically once they land in `agents/` — the browser reads whatever is in the repo at request time, so adding an agent is a PR, not a release.

**Naming convention**: agent names are noun-phrase role names that describe *who you're talking to* (`bug-hunter`, `software-architect`, `journal-coach`). This matches the loaded-persona UX (the name appears in the REPL prompt as `[bug-hunter] ❯`) and keeps agents clearly distinct from skill names, which are imperative-ish action verbs (`review`, `write-tests`, `inspect-cert` — see `skill-templates.md`).

**Categories**: re-use the same category set as `skill-templates.md` so the browse UI looks consistent across agents and skills. Empty or thin categories (e.g. `learning` with just `tutor`) are acceptable — they'll fill in as the curated set grows.

## Open questions

- Fixed category list vs. free-form? A fixed list keeps the browser tidy; free-form gives users more room. Compromise: define a fixed set for official agents, allow free-form on user agents, and group unknown values under "Other."
- Some agents overlap heavily with skills a user might invoke (`writer` vs `/scribe-meeting`, `security-auditor` vs `/audit-deps`). That's intentional — session persona vs single-turn procedure — but the browse UIs for agents and skills should make the distinction visible so users pick the right shape for their need.
- Should `kitesurfing-adivsor` keep its typo (pre-existing in the repo) or land corrected as `kitesurfing-advisor`? Lean toward corrected when the template first ships, since pull overwrites would propagate the typo into every user's `~/.aictl/agents/`.
- Should pure-prompt agents (`threat-modeler`, `journal-coach`, `psychologist`) be marked in frontmatter as `tools: none` so the REPL can skip tool-approval plumbing for them? Nice polish, not load-bearing for v1.
- Which categories should appear in the Browse UI when nothing is pulled yet — show with zero counts or hide? Defer until there's real feedback.

## Out of scope for v1

- Agents that need bundled resources (e.g. a `rust-expert` with accompanying reference sheets or cheat-sheets alongside the main prompt). Revisit when the core layout lifts the single-file restriction.
- Cross-agent composition (loading two agents at once, or one agent deferring to another). Users can describe handoffs inside the prompt.
- Per-language or per-framework variants (e.g. `rust-expert` vs `python-expert` vs `go-expert`). Start with `tutor` and domain-agnostic experts; fork into language-specific agents only if demand shows up.
- Community catalogue of third-party agents. Same stance as `agents-remote-catalogue.md` — only the official aictl repo is browsable in v1.
