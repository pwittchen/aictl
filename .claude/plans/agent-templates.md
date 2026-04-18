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

## Initial set

Chosen to cover distinct workflows and exercise different tool clusters.

### Dev & workflow

1. **`code-reviewer`** — reviews staged/unstaged changes. Leans on `git diff`, `read_file`, `lint_file`, `diff_files`. Focus: correctness, security, style; flags issues, doesn't rewrite unless asked.

2. **`commit-bot`** — reads `git diff --cached` and writes a short, imperative commit message following the repo's conventions. No AI attribution lines.

3. **`researcher`** — answers questions with citations. Uses `search_web`, `fetch_url`, `extract_website`. Always includes source URLs.

4. **`data-analyst`** — works over CSV/JSON with `csv_query`, `json_query`, `calculate`. Returns a table plus a one-line takeaway.

5. **`shell-helper`** — explains or composes shell commands before running them. Prefers dry-runs; uses `system_info` / `list_processes` / `check_port` for diagnostics.

6. **`image-assistant`** — analyzes or generates images. Uses `read_image` for vision (screenshots, diagrams, OCR-style transcription, alt-text) and `generate_image` for quick illustrations or mockups.

7. **`test-writer`** — generates unit/integration tests for a target file and runs them via `run_code` or `exec_shell`.

8. **`refactorer`** — proposes small, reversible edits via `edit_file` + `diff_files`. Shows a diff before committing.

9. **`docs-writer`** — updates README / inline docs / project docs from the current code. Uses `read_document` for PDF/DOCX inputs.

10. **`debugger`** — reproduces a bug, narrows it down with prints/logs, proposes a minimal fix. Leans on `run_code`, `exec_shell`, `search_files`.

11. **`regex-helper`** — builds and explains regex patterns. Tests candidates against sample inputs via `run_code` before handing the final pattern back.

12. **`changelog-writer`** — generates CHANGELOG entries from `git log` between two refs. Groups changes by type (features / fixes / breaking) and skips noise (merge commits, version bumps).

13. **`prompt-engineer`** — refines and critiques LLM prompts. Asks for the target model and failure modes, then proposes a tightened prompt with a rationale for each change.

14. **`architect`** — discusses high-level design and tradeoffs. Deliberately does *not* write code — outputs options, constraints, and a recommendation with the main risks called out.

15. **`api-tester`** — pokes REST endpoints with `fetch_url` and filters responses through `json_query`. Uses `check_port` to sanity-check reachability before sending requests.

### Ops

16. **`sysadmin`** — machine diagnostics via `system_info`, `list_processes`, `check_port`. Uses `notify` on long-running completions.

17. **`log-sleuth`** — tails, greps, and summarizes logs for incident triage. Combines `exec_shell`, `search_files`, and `read_file`.

18. **`docker-operator`** — manages Docker containers, images, and Compose stacks via `exec_shell` (`docker ps`, `docker images`, `docker build`, `docker compose up/down/logs`). Reads and edits Dockerfiles and `docker-compose.yml`; uses `check_port` to verify published ports and `list_processes` to spot host-side conflicts. Prefers dry-runs and explains destructive commands (`rm`, `prune`, `down -v`) before running them.

### Security

19. **`security-auditor`** — greps for secrets, risky patterns, and unsafe APIs; runs dependency audits via `exec_shell`. Flags issues without auto-fixing.

### Learning

20. **`tutor`** — explains concepts at the requested level and produces small runnable examples via `run_code`.

### Knowledge work

21. **`writer`** — drafts and tightens prose from a brief. Uses `read_document` for source material and `clipboard` to hand output back.

22. **`editor`** — line-edits existing text for clarity and tone. Shows before/after; good for emails, posts, docs.

23. **`summarizer`** — condenses long documents, articles, or URLs into a fixed shape (TL;DR + bullets). Pairs `read_document` with `extract_website`.

24. **`translator`** — translates between languages with a short note on tone/register choices.

25. **`copywriter`** — writes marketing and product copy (taglines, landing-page sections, ad copy, release notes) from a brief. Tunes voice and length to the channel; offers a few variants when asked.

26. **`meeting-notes`** — turns a meeting transcript into a structured summary with decisions and action items (owner + due date when stated). Pairs `read_document` with `clipboard`.

27. **`email-writer`** — drafts emails with tone and length targeting (short / formal / follow-up / cold outreach / reply). Asks for the goal and recipient context before writing.

### Daily life

28. **`chef`** — suggests recipes from a list of ingredients on hand, respecting dietary constraints and time budget. Outputs steps plus a shopping delta for anything missing.

29. **`meal-planner`** — builds a weekly meal plan and a consolidated shopping list. Pairs well with `chef` for recipe depth.

30. **`travel-planner`** — drafts itineraries from a destination, dates, and budget. Uses `search_web` and `extract_website` for up-to-date info on venues, transit, and opening hours.

### Thinking & habits

31. **`decision-helper`** — applies structured frameworks (pro/con, weighted-criteria, regret-minimization) to a concrete choice. Surfaces assumptions and what would change the answer.

32. **`habit-coach`** — helps design a small, trackable habit with a cue, routine, reward, and check-in cadence. Keeps it modest on purpose — one habit at a time.

33. **`critic`** — cold, objective, and blunt. Stress-tests ideas, plans, and arguments by identifying weak assumptions, missing evidence, logical gaps, and likely failure modes. Will say "this is wrong" or "this won't work" and explain why, with no flattery or hedging. Not rude for rudeness's sake — reasoning is always shown. Useful as a counterweight to the usual LLM agreeableness and as a natural pair with `brainstormer` for a generate-then-critique loop.

### Creative & personal

34. **`brainstormer`** — generates wide-then-narrow idea lists. Enforces "no self-critique until round two" so the first pass stays generative.

35. **`journal-coach`** — asks reflective questions in a warm, non-judgmental tone. Pure-prompt, minimal tool use.

36. **`psychologist`** — a supportive conversational persona that draws on psychology-informed techniques (active listening, validation, gentle CBT-style reframing, open-ended questions). Explicitly not a substitute for professional mental health care — includes a standing instruction to suggest reaching out to a qualified professional or crisis line when the user describes acute distress, self-harm, or harm to others. Pure-prompt, no tool use.

37. **`storyteller`** — writes short fiction from a premise. Tunable length, genre, and POV; asks clarifying questions before drafting long pieces.

38. **`poet`** — writes poems in a requested form (haiku, sonnet, free verse, limerick, …) with a short note on the form choice and any constraints obeyed.

## Design sketch

**Storage**: templates live in-tree under `assets/agents/*.md` (or similar), compiled into the binary via `include_str!`.

**Installation**: on first run, if `~/.aictl/agents/` is empty (or missing), write the bundled templates to disk. Skip any filename that already exists so user customizations are never overwritten. A `--install-agent-templates` CLI flag (and maybe a `/agent install-templates` menu entry) forces a re-copy, also skipping existing names.

**Discovery**: the existing `/agent` view-all menu already lists everything in `~/.aictl/agents/`, so templates appear automatically with no UI changes. The `--list-agents` flag lists them too.

**Categories**: agents carry an optional category (e.g. `dev`, `ops`, `security`, `learning`, `knowledge-work`, `creative`). For bundled templates the category is fixed in the asset's frontmatter; for user-authored agents it's an optional field editable from `/agent`. Agents without a category fall into an `uncategorized` bucket. In the interactive `/agent` browse view the user can pick **All** to see every agent in one flat list (current behavior) or drill into a specific category first. The category browser lists categories with a count next to each (e.g. `dev (10)`, `ops (2)`) and opens into the same row UI used today. The `--list-agents` CLI flag gains an optional `--category <name>` filter.

**Removal**: user deletes like any other agent via `/agent` or `rm`.

## Open questions

- Should templates be marked (e.g. a `# Built-in template` header comment) so users can tell ours apart from their own?
- Do we want a single manifest file listing the bundled templates, or is globbing `assets/agents/*.md` at build time enough?
- Should `--install-agent-templates` prompt before overwriting, or always skip existing?
- How is the category stored on user-authored agents? Options: frontmatter at the top of the prompt file, a sidecar `<name>.meta` file, or a single `~/.aictl/agents/.categories` index. Frontmatter is closest to the plain-text ethos but means the prompt file is no longer "just the prompt."
- Fixed category list vs. free-form? A fixed list keeps the browser tidy; free-form gives users more room. Compromise: ship a fixed set for built-ins, allow free-form on user agents, and group unknown values under "Other."

## Out of scope for v1

- Community template registry / remote install.
- Per-template metadata (tags, description, recommended model).
- Template versioning / upgrade flow.
