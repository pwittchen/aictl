# Plan: Remote skills catalogue

## Context

The core skills feature (see `skills.md`) lets users codify and invoke repeatable markdown procedures on demand — but every skill has to be written by the user. Discovery and sharing are manual: copy-paste from a README, retype from memory, or scrape someone else's dotfiles.

This plan extends skills with a first-party catalogue: aictl ships with a curated set of **official skills** that live in the project's GitHub repo under `.aictl/skills/` and can be browsed and pulled on demand from the REPL, without being compiled into the binary. New skills land in the catalogue the moment they're merged to master — users pick them up on the next Browse open, no release needed.

Prerequisite: the core skills feature from `skills.md` is shipped. Every assumption below — directory layout, frontmatter parser, `/skills` menu, `src/skills.rs` module, `--skill` / `--list-skills` flags — exists and is stable.

## Goals & Non-goals

**Goals**
- Ship a curated, first-party set of official skills (e.g. `commit`, `review`, `summarize-logs`) maintained in the project git repo under `.aictl/skills/<name>/SKILL.md`.
- **Do not** bundle the catalogue into the binary. The list is fetched dynamically from GitHub so new skills can ship without a release.
- Let users browse the catalogue from `/skills`, see which skills are already pulled, pull new ones, and re-pull to update.
- Make it obvious whether a skill on disk came from the official catalogue or was user-authored — a visible `[official]` badge.
- Work without a GitHub token. Public-repo reads only; 60/hr unauthenticated is enough for browse-then-pull.

**Non-goals**
- Community / third-party catalogues. Only the official aictl repo's `.aictl/skills/` directory is browsable. No arbitrary URLs, no additional sources.
- Signature verification on pulled skill bodies — we trust the repo the same way we trust the binary.
- Background auto-updates of already-pulled skills. Updates are user-initiated.
- Skill versioning beyond "pull overwrites" — no rollback, no history, no pinning.
- Bundled resources alongside `SKILL.md` (scripts, templates). Same deferral as the core skills plan; when it lands, the pull flow will switch from single-file to whole-directory fetches.

## Design

### 1. Source of truth

Official skills live in the project git repo under `.aictl/skills/<name>/SKILL.md`, one directory per skill — same layout as `~/.aictl/skills/` on disk so pulls are a straight copy. **Not** compiled into the binary: no `include_str!`, no bundled assets.

Repo coordinates (`owner`, `repo`, `branch`) are compile-time constants — the *list* of skills is dynamic, the *source location* is fixed. This keeps the catalogue reachable from every build without baking its contents into any specific release.

### 2. Frontmatter additions

Two new optional fields are accepted in `SKILL.md` frontmatter. The core parser already ignores unknown keys (see the core plan §2), so existing clients handle catalogue skills fine — they just miss the badging.

```markdown
---
name: commit
description: Commit staged changes with a clear, project-style message.
source: aictl-official
category: dev
---
```

- `source` — `aictl-official` for skills pulled from the project's GitHub repo; omitted (or `user`) for user-authored skills. The REPL and `--list-skills` render an `[official]` badge on rows whose frontmatter has `source: aictl-official`, so users can tell at a glance which skills came from the app and which they wrote themselves. Users can edit or remove the marker freely — there's nothing enforcing it beyond the badge.
- `category` — optional grouping key (`dev`, `ops`, `security`, …). Used by the browse/list UIs for drill-down.

### 3. Browse entry in `/skills`

The core menu (Create manually / Create with AI / View all / Cancel) gains a new **Browse official skills** entry, placed third:

1. Create manually
2. Create with AI
3. **Browse official skills**  ← new
4. View all
5. Cancel

Selecting it opens the remote browser.

### 4. Browse mechanics

Opening Browse fetches the skills directory listing from GitHub at request time — no hardcoded manifest. Two fetch paths, in order:

1. GitHub REST: `GET https://api.github.com/repos/<owner>/<repo>/git/trees/<branch>?recursive=1` returns every file in the repo; filter entries under `.aictl/skills/` to get the list. Using a single `git_trees` call (rather than `/contents/.aictl/skills` then one request per subdirectory) avoids N+1 requests.
2. For each skill's frontmatter, fetch the raw `SKILL.md` via `https://raw.githubusercontent.com/<owner>/<repo>/<branch>/.aictl/skills/<name>/SKILL.md` and parse.

No API key is required for public-repo reads; rate limits (60/hr unauthenticated) are acceptable for this browse-then-pull flow.

### 5. Pull flow

Selecting a skill in the browser downloads its `SKILL.md` to `~/.aictl/skills/<name>/SKILL.md`, creating the directory if needed. If a `SKILL.md` with that name already exists, the REPL prompts `Skill <name> already exists. Overwrite? [y/N]` before writing.

A `--pull-skill <name>` CLI flag mirrors the menu for non-interactive use; `--pull-skill <name> --force` skips the prompt.

v1 pulls only `SKILL.md`. When bundled resources land (see Non-goals), the pull flow will need to enumerate the whole directory and fetch every file.

### 6. Update indicator

The browse UI tags each row with state:
- `[ ]` — not yet pulled
- `[✓]` — already on disk, matches upstream
- `[↑]` — already on disk, upstream is newer (differing content)

Pulling an `[↑]` row re-downloads and overwrites (still prompts unless the user opts into a session-wide "update all" action). Detection is content-hash based: SHA-256 of the local `SKILL.md` against the upstream blob SHA exposed by the trees API; fall back to byte-for-byte diff if needed.

### 7. Discovery of installed skills

The existing "View all" entry continues to list everything in `~/.aictl/skills/` exactly as the core plan specified. The only UI change is the `[official]` badge on rows whose frontmatter has `source: aictl-official`. `--list-skills` adds the same badge.

### 8. Removal

Unchanged from the core plan. User deletes like any other skill via `/skills` or `rm -rf ~/.aictl/skills/<name>`. Deletion works regardless of whether the skill was pulled or user-authored — there's nothing sacred about official skills on disk.

### 9. Integration points

| File | Change |
|------|--------|
| `src/skills.rs` | Extend frontmatter parser to recognize `source` and `category`; `Skill` struct gains two fields |
| `src/skills/remote.rs` | **New** — GitHub trees listing + raw pull with overwrite prompt; returns `Vec<RemoteSkill>` with `name`, `description`, `category`, upstream blob SHA, and local `State::{NotPulled, UpToDate, UpstreamNewer}` |
| `src/commands/skills.rs` | Add "Browse official skills" entry; wire into the browse UI |
| `src/main.rs` | Add `--pull-skill <name>` + `--force` flags |
| `.aictl/skills/` | **New in-repo directory** — one subdirectory per official skill, each containing `SKILL.md` with frontmatter including `source: aictl-official` |
| `CLAUDE.md` | Short addition describing the remote catalogue and how the `source: aictl-official` marker works |
| `ROADMAP.md` | Remove the corresponding entry once shipped |

## Rollout phases

1. **Phase 1** — Extend frontmatter parser (`source`, `category`); add `[official]` badge to `/skills` View all and `--list-skills`. No network yet.
2. **Phase 2** — `src/skills/remote.rs` with GitHub trees listing + raw pull; `--pull-skill` + `--force` flags so the feature is verifiable end-to-end without the browse UI.
3. **Phase 3** — "Browse official skills" menu entry with update indicators.
4. **Phase 4** — Seed `.aictl/skills/` in the repo with an initial official set (candidates: `commit`, `review`, `summarize-logs`, plus any from `.claude/skills/` worth promoting).
5. **Phase 5** — Docs: update README and `docs/skills.md` with the pull flow; short walkthrough / screenshot of the browse UI.

## Verification

1. `cargo build` / `cargo build --release` — clean.
2. `cargo lint` — no warnings.
3. `cargo test` — frontmatter parser recognizes `source` + `category`; remote listing + pull integration tests pass (against a mocked GitHub response).
4. Manual:
   - Open **Browse official skills**; confirm the list reflects the live contents of `.aictl/skills/` in the repo (add a skill in a branch, re-open, confirm it appears).
   - Pull an official skill; confirm `~/.aictl/skills/<name>/SKILL.md` lands on disk with `source: aictl-official` preserved.
   - Pull again over an existing skill; confirm the overwrite prompt fires — `No` preserves local edits, `Yes` replaces.
   - Edit a pulled skill locally; confirm the browse row switches to `[↑]` when upstream differs.
   - Non-interactive pull: `aictl --pull-skill commit` and `aictl --pull-skill commit --force`.
   - `/skills` → View all: confirm `[official]` badge on pulled skills and no badge on hand-authored ones.
   - No network / rate-limited: confirm Browse fails gracefully with a clear message (not a panic, not a stack trace).

## Open questions

- GitHub API (metadata-rich but rate-limited at 60/hr unauthenticated) vs. raw CDN (unlimited but no directory listing). A hybrid — list via API, fetch via raw — keeps most of both, but what happens when the API rate limit is exhausted mid-browse? Show cached list from last successful fetch?
- Should the browser cache the remote listing to disk (e.g. `~/.aictl/skills/.remote-cache.json` with a short TTL) so repeat opens don't re-hit GitHub, or always fetch fresh? Fresh is simpler; cached is friendlier to flaky connections.
- Update check trigger: on-demand (user opens Browse) or periodic (background refresh on REPL startup)? On-demand is simpler and respects the no-surprise-network-calls principle.
- Is the `[↑]` upstream-newer detection worth the extra fetch per row, or should we just show `[✓]` and let the user re-pull if they want the latest? Defer until there's real feedback.
- Should pulled skills be flagged immutable by default with a "fork to user" action for customization? Probably over-engineered for v1 — users can edit in place (the `source` marker stays unless they change it) or delete and recreate.
- Signature verification on pulled skill bodies? We trust the repo the same way we trust the binary. Probably not worth it for v1.
