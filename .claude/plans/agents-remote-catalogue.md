# Plan: Remote agents catalogue

## Context

The core `/agent` system lets users create, load, and manage session-long personas as plain markdown files under `~/.aictl/agents/<name>` — but every agent has to be written by the user. Discovery and sharing are manual: copy-paste from a README, retype from memory, or scrape someone else's dotfiles. The built-in set is empty, so new users start from a blank slate.

This plan extends the `/agent` system with a first-party catalogue: aictl ships with a curated set of **official agents** that live in the project's GitHub repo under `.aictl/agents/` and can be browsed and pulled on demand from the REPL, without being compiled into the binary. New agents land in the catalogue the moment they're merged to master — users pick them up on the next Browse open, no release needed.

Prerequisite: the core `/agent` system (see `src/agents.rs`, `src/commands/agent.rs`, `--agent` / `--list-agents` flags) exists and is stable. This plan layers onto it — nothing below requires changing how agents are loaded, only how they arrive on disk.

## Goals & Non-goals

**Goals**
- Ship a curated, first-party set of official agents (e.g. `bug-hunter`, `software-architect`, `researcher`) maintained in the project git repo under `.aictl/agents/<name>.md`.
- **Do not** bundle the catalogue into the binary. The list is fetched dynamically from GitHub so new agents can ship without a release.
- Let users browse the catalogue from `/agent`, see which agents are already pulled, pull new ones, and re-pull to update.
- Make it obvious whether an agent on disk came from the official catalogue or was user-authored — a visible `[official]` badge.
- Work without a GitHub token. Public-repo reads only; 60/hr unauthenticated is enough for browse-then-pull.

**Non-goals**
- Community / third-party catalogues. Only the official aictl repo's `.aictl/agents/` directory is browsable. No arbitrary URLs, no additional sources.
- Signature verification on pulled agent bodies — we trust the repo the same way we trust the binary.
- Background auto-updates of already-pulled agents. Updates are user-initiated.
- Agent versioning beyond "pull overwrites" — no rollback, no history, no pinning.
- Bundled resources alongside an agent file (sidecar reference sheets, cheat-sheets). Agent = one markdown file in v1; when bundled resources land, the pull flow will switch from single-file to whole-directory fetches.

## Design

### 1. Source of truth

Official agents live in the project git repo under `.aictl/agents/<name>.md`, one file per agent — same layout as `~/.aictl/agents/` on disk so pulls are a straight copy. **Not** compiled into the binary: no `include_str!`, no bundled assets.

Repo coordinates (`owner`, `repo`, `branch`) are compile-time constants — the *list* of agents is dynamic, the *source location* is fixed. This keeps the catalogue reachable from every build without baking its contents into any specific release.

### 2. Frontmatter additions

Two new optional fields are accepted in the agent frontmatter block. The existing agent loader already tolerates frontmatter (or falls through when the file is plain prose), so existing clients handle catalogue agents fine — they just miss the badging.

```markdown
---
name: bug-hunter
description: Reproduces a bug, narrows it down, proposes a minimal fix.
source: aictl-official
category: dev
---

You are a bug hunter...
```

- `source` — `aictl-official` for agents pulled from the project's GitHub repo; omitted (or `user`) for user-authored agents. The REPL and `--list-agents` render an `[official]` badge on rows whose frontmatter has `source: aictl-official`, so users can tell at a glance which agents came from the app and which they wrote themselves. Users can edit or remove the marker freely — there's nothing enforcing it beyond the badge.
- `category` — optional grouping key (`dev`, `ops`, `security`, `learning`, `knowledge-work`, `data`, `daily-life`, `thinking-habits`, `creative`, …). Used by the browse/list UIs for drill-down. Agents without a category fall into an `uncategorized` bucket.

### 3. Browse entry in `/agent`

The existing `/agent` menu (Create manually / Create with AI / View all / Cancel) gains a new **Browse official agents** entry, placed third:

1. Create manually
2. Create with AI
3. **Browse official agents**  ← new
4. View all
5. Cancel

Selecting it opens the remote browser.

### 4. Browse mechanics

Opening Browse fetches the agents directory listing from GitHub at request time — no hardcoded manifest. Two fetch paths, in order:

1. GitHub REST: `GET https://api.github.com/repos/<owner>/<repo>/git/trees/<branch>?recursive=1` returns every file in the repo; filter entries under `.aictl/agents/` to get the list. Using a single `git_trees` call (rather than `/contents/.aictl/agents` then one request per file) avoids N+1 requests.
2. For each agent's frontmatter, fetch the raw `.md` via `https://raw.githubusercontent.com/<owner>/<repo>/<branch>/.aictl/agents/<name>.md` and parse.

No API key is required for public-repo reads; rate limits (60/hr unauthenticated) are acceptable for this browse-then-pull flow.

The category browser lists categories with a count next to each (e.g. `dev (11)`, `ops (3)`) and opens into the same row UI used for flat lists. An **All** option shows every agent in one flat list.

### 5. Pull flow

Selecting an agent in the browser downloads its `.md` to `~/.aictl/agents/<name>`. If a file with that name already exists, the REPL prompts `Agent <name> already exists. Overwrite? [y/N]` before writing.

A `--pull-agent <name>` CLI flag mirrors the menu for non-interactive use; `--pull-agent <name> --force` skips the prompt.

v1 pulls only a single `.md`. When bundled resources land (see Non-goals), the pull flow will need to enumerate a per-agent directory and fetch every file.

### 6. Update indicator

The browse UI tags each row with state:
- `[ ]` — not yet pulled
- `[✓]` — already on disk, matches upstream
- `[↑]` — already on disk, upstream is newer (differing content)

Pulling an `[↑]` row re-downloads and overwrites (still prompts unless the user opts into a session-wide "update all" action). Detection is content-hash based: SHA-256 of the local agent file against the upstream blob SHA exposed by the trees API; fall back to byte-for-byte diff if needed.

### 7. Discovery of installed agents

The existing "View all" entry continues to list everything in `~/.aictl/agents/` exactly as today. The only UI change is the `[official]` badge on rows whose frontmatter has `source: aictl-official`. `--list-agents` adds the same badge, and gains an optional `--category <name>` filter.

### 8. Removal

Unchanged from the core `/agent` flow. User deletes like any other agent via `/agent` or `rm ~/.aictl/agents/<name>`. Deletion works regardless of whether the agent was pulled or user-authored — there's nothing sacred about official agents on disk.

### 9. Integration points

| File | Change |
|------|--------|
| `src/agents.rs` | Extend frontmatter parser to recognize `source` and `category`; agent struct gains two fields |
| `src/agents/remote.rs` | **New** — GitHub trees listing + raw pull with overwrite prompt; returns `Vec<RemoteAgent>` with `name`, `description`, `category`, upstream blob SHA, and local `State::{NotPulled, UpToDate, UpstreamNewer}` |
| `src/commands/agent.rs` | Add "Browse official agents" entry; wire into the browse UI; add category drill-down |
| `src/main.rs` | Add `--pull-agent <name>` + `--force` flags; `--list-agents --category <name>` filter |
| `.aictl/agents/` | **New in-repo directory** — one `.md` per official agent, each with frontmatter including `source: aictl-official` (populated by `agent-templates.md`) |
| `CLAUDE.md` | Short addition describing the remote catalogue and how the `source: aictl-official` marker works |
| `ROADMAP.md` | Remove the corresponding entry once shipped |

## Rollout phases

1. **Phase 1** — Extend frontmatter parser (`source`, `category`); add `[official]` badge to `/agent` View all and `--list-agents`. No network yet.
2. **Phase 2** — `src/agents/remote.rs` with GitHub trees listing + raw pull; `--pull-agent` + `--force` flags so the feature is verifiable end-to-end without the browse UI.
3. **Phase 3** — "Browse official agents" menu entry with update indicators and category drill-down.
4. **Phase 4** — Seed `.aictl/agents/` in the repo with the initial official set from `agent-templates.md`.
5. **Phase 5** — Docs: update README and `docs/agents.md` with the pull flow; short walkthrough / screenshot of the browse UI.

## Verification

1. `cargo build` / `cargo build --release` — clean.
2. `cargo lint` — no warnings.
3. `cargo test` — frontmatter parser recognizes `source` + `category`; remote listing + pull integration tests pass (against a mocked GitHub response).
4. Manual:
   - Open **Browse official agents**; confirm the list reflects the live contents of `.aictl/agents/` in the repo (add an agent in a branch, re-open, confirm it appears).
   - Drill into a category; confirm the counts match the flat-list total.
   - Pull an official agent; confirm `~/.aictl/agents/<name>` lands on disk with `source: aictl-official` preserved.
   - Pull again over an existing agent; confirm the overwrite prompt fires — `No` preserves local edits, `Yes` replaces.
   - Edit a pulled agent locally; confirm the browse row switches to `[↑]` when upstream differs.
   - Non-interactive pull: `aictl --pull-agent bug-hunter` and `aictl --pull-agent bug-hunter --force`.
   - `/agent` → View all: confirm `[official]` badge on pulled agents and no badge on hand-authored ones.
   - Load a pulled agent; confirm it behaves as a normal session persona (prompt shows `[name] ❯`, `/agent unload` clears it).
   - No network / rate-limited: confirm Browse fails gracefully with a clear message (not a panic, not a stack trace).

## Open questions

- YAML frontmatter vs. a sidecar `<name>.meta` file vs. a single `~/.aictl/agents/.index`. Frontmatter is closest to the plain-text ethos but means the prompt file is no longer "just the prompt." Current leaning: frontmatter, because it round-trips cleanly when pulled from GitHub and doesn't require a parallel file per agent.
- GitHub API (metadata-rich but rate-limited at 60/hr unauthenticated) vs. raw CDN (unlimited but no directory listing). A hybrid — list via API, fetch via raw — keeps most of both, but what happens when the API rate limit is exhausted mid-browse? Show cached list from last successful fetch?
- Should the browser cache the remote listing to disk (e.g. `~/.aictl/agents/.remote-cache.json` with a short TTL) so repeat opens don't re-hit GitHub, or always fetch fresh? Fresh is simpler; cached is friendlier to flaky connections.
- Update check trigger: on-demand (user opens Browse) or periodic (background refresh on REPL startup)? On-demand is simpler and respects the no-surprise-network-calls principle.
- Is the `[↑]` upstream-newer detection worth the extra fetch per row, or should we just show `[✓]` and let the user re-pull if they want the latest? Defer until there's real feedback.
- Fixed category list vs. free-form? A fixed list keeps the browser tidy; free-form gives users more room. Compromise: define a fixed set for official agents, allow free-form on user agents, and group unknown values under "Other."
- Should pulled agents be flagged immutable by default with a "fork to user" action for customization? Probably over-engineered for v1 — users can edit in place (the `source` marker stays unless they change it) or delete and recreate.
