# Plan: Skills for aictl

## Context

Today aictl users can extend behavior in two ways: **agents** (persistent system-prompt extensions that stay loaded for a whole session) and raw prompts typed at the REPL. Neither fits the case where a user has a repeatable *procedure* — "review the pending diff," "write a commit message the way I like it," "summarize these logs the standard way" — that they want to invoke on demand without carrying the persona around for the rest of the session.

Skills fill that gap: markdown playbooks stored on disk, invoked explicitly via a slash command, injected as instructions for a single turn, then gone. The LLM still executes using its normal tool access — skills encode *expertise and procedure* the LLM should follow, not deterministic scripts.

## Goals & Non-goals

**Goals**
- Let users codify repeatable procedures as markdown files and invoke them on demand.
- Scope a skill invocation to **one turn**, not the whole session. That's the key differentiator from agents.
- Mirror the existing `/agent` UX patterns so users already familiar with agents pick this up instantly.
- Support both interactive (`/<skill-name>` in REPL) and single-shot (`--skill <name>`) invocation.
- Ship a curated, first-party set of official skills that users can browse and pull on demand from the project's GitHub repo. The catalog is *not* bundled into the binary, so it can grow without a release.

**Non-goals**
- No automatic / context-sensitive skill invocation in v1 (the LLM deciding which skill to load based on the user's message). Explicit invocation only — easier to debug, no magic.
- No bundled resources (scripts, templates) alongside the skill markdown in v1. Skill = one markdown file. Revisit if users ask for it.
- No skill chaining / composition (a skill invoking another skill). Users can describe sequences inside the markdown.
- Skills are not tools, plugins, or agents. Keep the mental model clean — four distinct extension points, each with one job.

## How it differs from agents (and why both exist)

|                  | Agent                               | Skill                                 |
|------------------|-------------------------------------|---------------------------------------|
| Scope            | Whole session                       | Single turn                           |
| Lifetime         | Persists until `/agent unload`      | Gone after the turn completes         |
| Invocation       | `--agent <name>` / `/agent`         | `/<skill-name>` / `--skill <name>`    |
| Storage          | `~/.aictl/agents/<name>` (plain)    | `~/.aictl/skills/<name>/SKILL.md`     |
| REPL indicator   | `[agent-name] ❯` in prompt          | None (transient)                      |
| Purpose          | "Be a Rust expert for this session" | "Run the standard commit procedure"   |
| Concrete example | `rust-expert`, `tech-writer`        | `commit`, `review`, `summarize-logs`  |

An agent sets *who you're talking to*. A skill tells that interlocutor *what to do next*. They compose — you can be mid-session with `rust-expert` loaded and still invoke `/commit`.

---

## Design

### 1. Layout on disk

```
~/.aictl/skills/
├── commit/
│   └── SKILL.md
├── review/
│   └── SKILL.md
└── summarize-logs/
    └── SKILL.md
```

One directory per skill, each containing a `SKILL.md`. Directory name = skill name, validated with the same rules as agent names (alphanumeric + `_`/`-`, reuse `is_valid_name` from `agents.rs` by promoting it to a shared `names.rs` or duplicating — favor duplication to avoid premature abstraction).

The directory form (rather than a single `~/.aictl/skills/<name>.md` file) is a small tax today that pays off later if bundled resources get added; changing the layout after launch would be disruptive.

### 2. Frontmatter

```markdown
---
name: commit
description: Commit staged changes with a clear, project-style message.
source: aictl-official
category: dev
---

When the user asks you to commit:

1. Run `git status` and `git diff --cached` to see what's staged.
2. ...
```

Fields:
- `name` — must match the directory name, re-validated at load.
- `description` — one-line summary shown in `/skills` listings. Becomes the auto-invocation hook if we add it later; for now it's purely informational.
- `source` — `aictl-official` for skills pulled from the project's GitHub repo; omitted (or `user`) for user-authored skills. The REPL and `--list-skills` render an `[official]` badge on rows whose frontmatter has `source: aictl-official`, so users can tell at a glance which skills came from the app and which they wrote themselves.
- `category` — optional grouping key (`dev`, `ops`, `security`, …). Used by the browse/list UIs for drill-down.

Frontmatter parsing: a simple hand-rolled YAML subset parser (key-value lines between `---` fences) keeps us off the `serde_yaml` dependency. Only `name`, `description`, `source`, and `category` are recognized in v1; unknown fields are silently ignored so the format can grow without breaking older clients.

### 3. Invocation

**Interactive REPL**:
- `/<skill-name>` — invoke the skill. After the slash-command dispatcher in `commands.rs` fails to match a built-in, fall through to `skills::find(name)`. If found, the skill's body (frontmatter stripped) is injected as an additional system message for the *next* user turn, and the REPL prompts for the task.
- `/<skill-name> <task>` — inline task form. Skill body injected + task forwarded immediately as the user message. Same outcome, one fewer round trip.
- `/skills` — menu to list/view/create/delete skills. Mirrors `/agent` exactly.

**Single-shot CLI**:
- `aictl --skill <name> --message "..."` — inject the skill and run the message. Works with and without `--auto`.
- `aictl --list-skills` — non-interactive listing.

### 4. Prompt injection mechanics

When a skill is invoked, `run_agent_turn` gets called with an extra transient system message before the user message:

```
[system] {base system prompt + optional AICTL.md + optional agent}
[system] # Skill: commit
         {skill body}
[user]   {user message}
```

Key property: the skill message is **not persisted into session history**. Session save writes the user message and the assistant reply but drops the injected skill block. Rationale: a session loaded later should replay faithfully — if the skill is gone/edited, we don't want stale instructions hanging around. Storing the skill *name* in session metadata as an annotation is fine (lets `--list-sessions` show "session X used skill `commit`") but the body is not persisted.

Only the current turn's LLM call sees the skill content. Subsequent turns in the same session revert to the plain system prompt unless the user invokes the skill again.

### 5. UX: create, browse, pull (`/skills` menu)

Five entries:
1. **Create manually** — prompts for name, description, then multi-line markdown body. Writes `SKILL.md`.
2. **Create with AI** — prompts for a one-line goal; the LLM drafts a full skill markdown (frontmatter + body). Reuse the AI-drafting helper from `/agent create with AI`.
3. **Browse official skills** — fetches the skills directory listing from the project's GitHub repo dynamically; selecting a row pulls its `SKILL.md` to disk.
4. **View all** — arrow-key list of locally installed skills; selecting one offers view / delete / invoke now. Rows with `source: aictl-official` in frontmatter get an `[official]` badge.
5. **Cancel**.

**Source of truth for official skills**: they live in the project git repo under `skills/<name>/SKILL.md`, one directory per skill — same layout as `~/.aictl/skills/` on disk. **Not** compiled into the binary: no `include_str!`, no bundled assets. The browse list is fetched at runtime so new skills can ship without cutting a release.

**Browse & pull mechanics**: selecting "Browse official skills" fetches the directory listing from GitHub at request time — no hardcoded manifest. Two fetch paths, in order:

1. GitHub REST: `GET https://api.github.com/repos/<owner>/<repo>/contents/skills?ref=master` returns one entry per subdirectory; a single `git_trees` call with `?recursive=1` is preferred to avoid N+1 requests when fetching each skill's frontmatter.
2. Fallback: raw `https://raw.githubusercontent.com/<owner>/<repo>/master/skills/<name>/SKILL.md` for individual pulls.

The repo coordinates (`owner`, `repo`, `branch`) are constants in the binary — the *list* is dynamic, the *source location* is fixed. No API key is required for public-repo reads; rate limits (60/hr unauthenticated) are acceptable for this browse-then-pull flow.

**Pull flow**: selecting a skill in the browser downloads its `SKILL.md` to `~/.aictl/skills/<name>/SKILL.md`, creating the directory if needed. If a `SKILL.md` with that name already exists, the REPL prompts `Skill <name> already exists. Overwrite? [y/N]` before writing. A `--pull-skill <name>` CLI flag mirrors the menu for non-interactive use; `--pull-skill <name> --force` skips the prompt. v1 pulls only `SKILL.md` — when bundled resources land (see Open questions), the pull flow fetches the whole directory tree.

**Update indicator**: the browse UI tags each row with state:
- `[ ]` — not yet pulled
- `[✓]` — already on disk, matches upstream
- `[↑]` — already on disk, upstream is newer (differing content)

Pulling an `[↑]` row re-downloads and overwrites (still prompts unless the user opts into a session-wide "update all" action). Detection is content-hash based: SHA-256 of the local `SKILL.md` against the upstream blob SHA; fall back to byte-for-byte diff if needed.

### 6. Slash-command collision handling

The existing slash-command set is fixed (`/agent`, `/behavior`, `/clear`, `/compact`, `/config`, `/context`, `/copy`, `/exit`, `/gguf`, `/help`, `/info`, `/keys`, `/memory`, `/mlx`, `/model`, `/security`, `/session`, `/skills`, `/stats`, `/tools`, `/uninstall`, `/update`, `/version`). A skill with one of those names must be rejected at creation time with a clear error (reserved-name list lives next to the dispatcher).

The slash-command dispatcher already returns `CommandResult::Unknown` for unrecognized slashes; that branch becomes the skill lookup path. If the slash still doesn't match a skill, the existing "unknown command" error fires.

### 7. Sandbox & security

Skills don't grant new capabilities — they're prompt content, not code. The LLM uses its existing tool access, and every tool call still passes through `security::validate_tool()`. No new gate is needed.

Two caveats:
- **Prompt-injection surface**: skill bodies are trusted (the user wrote them). But if a user ever shares/receives a skill file from elsewhere, `SKILL.md` could contain jailbreak attempts. Not our problem to solve in v1 — the same is true of agents today — but worth noting in docs: "don't install skills you didn't write or review."
- **`security::detect_prompt_injection`** runs on user input but not on injected system messages. Leave it that way; the user explicitly asked for this skill, so scanning its body for "ignore previous instructions" would be user-hostile.

### 8. Config & opt-out

Skills are first-party and enabled by default (unlike plugins, which are third-party). One config key:

```
AICTL_SKILLS_DIR=~/.aictl/skills     # override for testing
```

No master on/off — if the user doesn't want skills, they don't create any, and the `/skills` menu and `--skill` flag become inert.

### 9. Integration points

| File | Change |
|------|--------|
| `src/skills.rs` | **New** — CRUD + frontmatter parsing (including `source`, `category`) + global invocation state (minimal; see §10) |
| `src/commands.rs` | Route unrecognized slashes through `skills::find`; add `/skills` command |
| `src/commands/skills.rs` | **New** — menu (create manually / with AI / browse / view / delete) |
| `src/skills/remote.rs` | **New** — GitHub directory listing + raw pull with overwrite prompt; returns `Vec<RemoteSkill>` with `name`, `description`, `category`, upstream blob SHA, and local `State::{NotPulled, UpToDate, UpstreamNewer}` |
| `src/main.rs` | `--skill <name>`, `--list-skills`, `--pull-skill <name>`, `--force`; wire skill body into `run_agent_turn` as a one-shot system message |
| `src/config.rs` | Add `AICTL_SKILLS_DIR` reader |
| `src/session.rs` | Optionally record skill name in session metadata (annotation only, not body) |
| `skills/` | **New in-repo directory** — one subdirectory per official skill, each containing `SKILL.md` with frontmatter including `source: aictl-official` |
| `CLAUDE.md` | One-paragraph addition describing `src/skills.rs` and how skills differ from agents |
| `ROADMAP.md` | Remove the corresponding entry once shipped |

### 10. Runtime shape

Unlike agents, skills need **no persistent global state** — they're one-turn-scoped. The cleanest shape is a per-turn function parameter: `run_agent_turn(..., skill: Option<&Skill>)`. The REPL captures the skill at slash-dispatch time, passes it once, done. No `Mutex<Option<Skill>>` needed.

`Skill` itself:
```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,      // markdown with frontmatter stripped
}

pub fn find(name: &str) -> Option<Skill>;
pub fn list() -> Vec<SkillEntry>;        // name + description, for menus/--list-skills
pub fn save(name, description, body) -> io::Result<()>;
pub fn delete(name) -> io::Result<()>;
```

### 11. Testing

- **Unit tests** (`src/skills.rs`):
  - Frontmatter parse: valid, missing `---` fences, missing `name`, `name` / dir mismatch, unknown fields (ignored).
  - `is_valid_name` reuse/duplication behaves the same as agents.
  - Reserved-name rejection at save time.
  - Round-trip: `save` → `find` returns identical body.
- **Integration tests** (once the mock-LLM test harness from the roadmap lands):
  - `--skill <name> --message "..."` injects the skill body and the user message, skill body is absent on the next turn in an interactive session.
  - Slash-command path: `/commit` with a matching skill triggers the skill; `/commit` with no matching skill yields "unknown command."
  - Session persistence: loading a session that was run with a skill shows only the user/assistant turns, no skill body.

### 12. Documentation

- New `docs/skills.md` covering: file layout, frontmatter, invocation, interaction with agents, safety notes.
- README: a short "Skills" section next to the existing "Agents" section.
- Example `SKILL.md` files in `examples/skills/` for `commit`, `review`, `summarize-logs` — reference only, not auto-installed.

---

## Rollout phases

1. **Phase 1** — `src/skills.rs` + frontmatter parsing (including `source`, `category`) + slash-command fall-through + `--skill` / `--list-skills` flags. Skill body injected into `run_agent_turn`.
2. **Phase 2** — `/skills` interactive menu (create manually, view, delete).
3. **Phase 3** — "Create with AI" entry reusing the agent AI-drafting helper.
4. **Phase 4** — Browse & pull: `src/skills/remote.rs`, "Browse official skills" menu entry, `--pull-skill` CLI flag, update indicators, and seeding the in-repo `skills/` directory with an initial official set.
5. **Phase 5** — Docs, examples, session-metadata annotation.
6. **Phase 6 (future, optional)** — Auto-invocation: inject skill *descriptions* (not bodies) into the system prompt, let the LLM request a skill body via a dedicated tool call. Deferred until there's a concrete demand; the magic-to-value ratio is high.

## Verification

1. `cargo build` and `cargo build --release` — clean.
2. `cargo lint` — no warnings.
3. `cargo test` — unit + integration pass.
4. Manual:
   - Create `commit` skill via `/skills` menu; invoke via `/commit` and via `/commit review the staged diff and propose a message`.
   - Invoke same skill via `aictl --skill commit --message "..."` in single-shot mode.
   - Confirm subsequent REPL turns don't carry the skill body (inspect session JSON on disk).
   - Create skill with reserved name `help` — rejected with clear error.
   - Delete skill via menu; confirm `/commit` now yields "unknown command."
   - Load a pre-existing session that used a skill; confirm replay has no skill body.
   - Open **Browse official skills**; confirm the list reflects the live contents of `skills/` in the repo (add a skill in a branch, re-open, confirm it appears).
   - Pull an official skill; confirm `~/.aictl/skills/<name>/SKILL.md` lands on disk with `source: aictl-official` preserved.
   - Pull again over an existing skill; confirm the overwrite prompt fires — `No` preserves local edits, `Yes` replaces.
   - Edit a pulled skill locally; confirm the browse row switches to `[↑]` when upstream differs.
   - Non-interactive pull: `aictl --pull-skill commit` and `aictl --pull-skill commit --force`.
   - `/skills` → View all: confirm `[official]` badge on pulled skills and no badge on hand-authored ones.

## Open questions

- Should skill bodies go through `termimad` rendering when viewed in the `/skills` menu? Small polish; yes, cheap to add.
- Should the `description` field be required or optional? Lean required — forces users to articulate the skill's purpose, and it's the hook for future auto-invocation.
- Claude Code skills support bundled resources alongside `SKILL.md` (scripts, templates). We're deferring that — revisit after launch if users ask. When we do add it, the directory layout we're choosing in v1 accommodates it cleanly, and the pull flow will need to switch from single-file to whole-directory fetches.
- GitHub API (metadata-rich but rate-limited at 60/hr unauthenticated) vs. raw CDN (unlimited but no directory listing). A hybrid — list via API, fetch via raw — keeps most of both, but what happens when the API rate limit is exhausted mid-browse? Show cached list from last successful fetch?
- Should the browser cache the remote listing to disk (e.g. `~/.aictl/skills/.remote-cache.json` with a short TTL) so repeat opens don't re-hit GitHub, or always fetch fresh? Fresh is simpler; cached is friendlier to flaky connections.
- Update check trigger: on-demand (user opens Browse) or periodic (background refresh on REPL startup)? On-demand is simpler and respects the no-surprise-network-calls principle.
- Is the `[↑]` upstream-newer detection worth the extra fetch per row, or should we just show `[✓]` and let the user re-pull if they want the latest? Defer until there's real feedback.
- Signature verification on pulled skill bodies? We trust the repo the same way we trust the binary. Probably not worth it for v1.
