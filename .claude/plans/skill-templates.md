# Plan: Built-in Skill Templates

## Context

aictl's core skills feature (see `skills.md`) lets users codify repeatable single-turn procedures as markdown files invoked via `/<skill-name>`. The remote catalogue (see `skills-remote-catalogue.md`) ships the browse-and-pull machinery: `/skills` can list official skills from the project's GitHub repo and write `SKILL.md` files to `~/.aictl/skills/<name>/` on demand.

Both of those plans cover *how*. This one covers *what*: the initial curated set of official skills that lands in `skills/` in the repo so the catalogue has real content worth browsing on day one.

These templates are **starting points**, not sacred. Users can pull, view, modify, re-pull, and delete them like any other skill. They're distinguishable from user-authored skills only by the `source: aictl-official` frontmatter key.

## Goals & Non-goals

**Goals**
- Ship a curated set of broadly useful official skills that exercise different parts of the tool surface.
- Each skill should be short (under ~30 lines), opinionated, and self-contained.
- Favor skills that are genuinely one-shot: one invocation, one concrete output.
- Keep skill names short and action-shaped (imperative verbs or verb-noun pairs) so `/<skill-name>` reads like a command.

**Non-goals**
- No new delivery mechanism. `skills-remote-catalogue.md` owns the browse/pull flow; this plan just seeds `skills/` in the repo.
- No bundled resources (scripts, templates) alongside each `SKILL.md`. Deferred with the rest of the core plan.
- No per-provider or per-model tuning — skills are plain markdown.
- Nothing session-shaped or persona-shaped. Multi-turn experts (`shell-expert`, `bug-hunter`, `psychologist`, …) live in `agent-templates.md`.

## Initial set

Chosen to cover distinct workflows and exercise different tool clusters. Categories mirror the set used in `agent-templates.md` so the browse UI feels consistent across both content types.

### Dev & workflow

- **`review`** — reviews staged/unstaged changes. Leans on `git diff`, `read_file`, `lint_file`, `diff_files`. Focus: correctness, security, style; flags issues, doesn't rewrite unless asked.

- **`write-tests`** — generates unit/integration tests for a target file and runs them via `run_code` or `exec_shell`.

- **`update-docs`** — updates README / inline docs / project docs from the current code. Uses `read_document` for PDF/DOCX inputs.

- **`write-changelog`** — generates CHANGELOG entries from `git log` between two refs. Groups changes by type (features / fixes / breaking) and skips noise (merge commits, version bumps).

- **`investigate-error`** — takes a stack trace, panic, or compiler error and locates the relevant frames via `search_files` + `read_file`, then proposes a minimal fix. Works with Rust panics, Python tracebacks, JS stacks, Go errors, and typed-language compiler output.

### Ops

- **`summarize-logs`** — tails, greps, and summarizes logs for incident triage. Combines `exec_shell`, `search_files`, and `read_file`. (This is one of the canonical skill examples in `skills.md`.)

- **`inspect-cert`** — inspects TLS/SSL certificates for expiry, chain validity, SNI issues, and weak ciphers. Uses `fetch_url` for the HTTPS handshake and `check_port` to confirm reachability first. Flags anything expiring within 30 days.

- **`inspect-disk`** — read-only disk usage diagnostician. Uses `exec_shell` (`du`, `find`) to locate the biggest directories and oldest files; suggests (but never runs) cleanup commands. Honors the CWD jail so it cannot probe outside the working directory.

### Network

- **`inspect-http`** — analyzes HTTP responses via `fetch_url`: status, redirect chains, headers (security — CSP, HSTS, CORS, cookie flags, `X-Frame-Options`; caching; compression). Useful for web-perf and security posture sweeps.

- **`inspect-dns`** — DNS resolution across record types (A/AAAA/MX/TXT/CNAME/NS/CAA), DNSSEC validation, and propagation checks via `dig` / `host` through `exec_shell`.

- **`diagnose-network`** — connectivity and latency diagnosis via `ping` / `traceroute` / `mtr` through `exec_shell`. Broader than `check_port` — catches MTU issues, asymmetric routing, and packet loss.

- **`inspect-sockets`** — lists open ports, listeners, and established connections via `ss` / `lsof` / `netstat` through `exec_shell`. Deeper than `check_port` — catches what's actually bound on the host, not just whether a remote endpoint answers.

- **`test-bandwidth`** — throughput and latency measurement via `speedtest-cli` / `iperf3` / `ping` through `exec_shell`. Useful for isolating "is my connection slow" from app-level issues.

- **`scan-wifi`** — lists nearby SSIDs, channels, signal strength, and security modes via `airport -s` on macOS or `iw dev … scan` / `nmcli` on Linux through `exec_shell`. Suggests the least-congested 2.4 / 5 GHz channel from the scan.

- **`audit-wifi`** — audits your own network: WPA version, password strength guidance, guest-network isolation, WPS status, and rogue AP detection from a scan. Explicitly read-only — no deauth or cracking tooling.

### Security

- **`audit-deps`** — runs `cargo audit` / `npm audit` / `pip-audit` / `bundle-audit` via `exec_shell` and summarizes findings by severity. Cross-references CVE IDs and highlights transitive dependencies that are hardest to update.

- **`scan-secrets`** — runs `gitleaks` / `trufflehog` via `exec_shell` and walks `git log -p` for secrets already committed to history, not just the working tree. Useful before open-sourcing a repo or rotating keys after a suspected leak.

- **`check-cves`** — cross-references lockfiles (`Cargo.lock`, `package-lock.json`, `poetry.lock`, `Gemfile.lock`, `go.sum`) against CVE databases via `search_web` + `fetch_url`. More current than `audit-deps`'s tool-based summary when the local advisory DB is stale; flags exploitability and fix availability.

### Knowledge work

- **`summarize`** — condenses long documents, articles, or URLs into a fixed shape (TL;DR + bullets). Pairs `read_document` with `extract_website`.

- **`translate`** — translates between languages with a short note on tone/register choices.

- **`scribe-meeting`** — turns a meeting transcript into a structured summary with decisions and action items (owner + due date when stated). Pairs `read_document` with `clipboard`.

- **`write-email`** — drafts emails with tone and length targeting (short / formal / follow-up / cold outreach / reply). Asks for the goal and recipient context before writing.

### Data & diagrams

- **`draw-diagram`** — produces diagrams from a description or from source code it reads with `read_file`. Renders as mermaid (flowcharts, sequence, ER, state, gantt) for GitHub / Notion / Obsidian, or as ASCII box-and-line for READMEs and code comments where mermaid isn't rendered (kept under 80 columns). Hands output back via `clipboard` ready to paste.

- **`read-diagram`** — pairs `read_image` with transcription: takes a screenshot or photo of an architecture, flow, or whiteboard diagram and produces a text or mermaid transcription plus a short summary. Natural pair with `draw-diagram` for round-tripping hand-drawn sketches.

### Daily life

- **`suggest-recipe`** — suggests recipes from a list of ingredients on hand, respecting dietary constraints and time budget. Outputs steps plus a shopping delta for anything missing.

- **`plan-meals`** — builds a weekly meal plan and a consolidated shopping list. Pairs well with `suggest-recipe` for recipe depth.

- **`recommend-books`** — recommendations based on what you've loved or bounced off, with a one-line "why this fits you" per pick. Uses `search_web` + `extract_website` when you want reviews or current availability.

- **`mindfulness`** — short guided breathing or grounding exercise on demand (1-minute, 5-minute, 10-minute). Pure-prompt, no tool use. Keeps instructions concrete and paced.

### Thinking & habits

- **`devils-advocate`** — argues the opposing position, however reasonable your proposal sounds: "if this is wrong, why?" — useful for red-teaming a decision or plan before committing to it.

- **`pre-mortem`** — imagines the current plan or project has failed six months from now and works backward to identify what went wrong. Surfaces risks earlier than a "what could go wrong?" prompt because it grants the failure and asks for causes.

- **`steelman`** — builds the strongest possible case for a position, including ones you disagree with. Mirror image of `devils-advocate`: useful for understanding opposing views fairly, preparing for debate, or checking whether your own view survives contact with its best counterargument.

- **`weekly-review`** — GTD-style weekly review prompter: inbox, projects, waiting-for, someday/maybe, calendar. Pure-prompt, ritualized. Good as a scheduled Friday check-in.

### Creative & personal

- **`write-poem`** — writes poems in a requested form (haiku, sonnet, free verse, limerick, …) with a short note on the form choice and any constraints obeyed.

## Design sketch

**Storage**: each skill lives in the project git repo under `skills/<name>/SKILL.md`, one directory per skill. Same layout as `~/.aictl/skills/` on disk so pulls are a straight copy.

**Frontmatter**: every bundled skill carries the official marker and category:

```markdown
---
name: review
description: Review staged/unstaged changes for correctness, security, style.
source: aictl-official
category: dev
---

When the user asks you to review...
```

The `source: aictl-official` key is what the REPL uses to render an `[official]` badge next to pulled skills. `category` drives the browse-UI drill-down. Both fields are already specified in `skills-remote-catalogue.md` — this plan just populates them.

**Delivery**: nothing new to build. The browse-and-pull machinery in `skills-remote-catalogue.md` picks these files up automatically once they land in `skills/` — the browser reads whatever is in the repo at request time, so adding a skill is a PR, not a release.

**Naming convention**: skill names are short, imperative-ish, and describe the *action* the skill performs (`review`, `write-tests`, `inspect-cert`). This matches the canonical examples from the core skills plan (`commit`, `review`, `summarize-logs`) and keeps invocation at the REPL feeling like a command (`/review`, `/write-tests <path>`). This is the main visible difference from `agent-templates.md` entries, which keep noun-phrase role names (`bug-hunter`, `software-architect`, `journal-coach`).

**Categories**: re-use the same category set as `agent-templates.md` so the browse UI looks consistent across agents and skills. When a skill is the only thing in a category (currently `creative-personal` with just `write-poem`), that's fine — empty rows are acceptable.

## Open questions

- Some skills overlap with slash commands a user might type anyway (`/review`, `/commit`). Should the core `/skills` dispatcher prefer skills over future built-in commands, or vice versa? Lean skills first — closer to user intent when they typed the slash. Already discussed in `skills.md` §6 (slash-command collision handling).
- Should pure-prompt skills (`mindfulness`, `steelman`, `pre-mortem`, `weekly-review`) be marked in frontmatter as `tools: none` so the REPL can skip tool-approval plumbing for them? Nice polish, not load-bearing for v1.
- Which categories should appear in the Browse UI when nothing is pulled yet — show with zero counts or hide? Defer until there's real feedback.
- Should the canonical examples in `skills.md` (`commit`, `review`, `summarize-logs`) all ship here? `review` and `summarize-logs` are in. `commit` is intentionally *not* listed — it already exists as a user-level convention (see `.claude/skills/commit/`) and the first-party version would likely disagree with users' house styles. Revisit if demand shows up.

## Out of scope for v1

- Skills that need bundled resources (e.g. `commit` with an example commit-message template alongside). Revisit when the core plan lifts the single-file restriction.
- Cross-skill composition (a skill invoking another skill). Users can describe sequences inside the markdown.
- Auto-invocation based on the user's message contents. Future optional core feature; not a per-skill concern.
- Per-language or per-framework variants (e.g. `write-tests-rust` vs `write-tests-python`). The single skill detects the language from the target file and adapts.
- Community catalogue of third-party skills. Same stance as `skills-remote-catalogue.md` — only the official aictl repo is browsable in v1.
