# Spec: Approval granularity — persistent tool rules

> Report item **10** (Tier 3). Should land after file checkpointing — broad grants are
> defensible when edits are revertable.

## Context

`ui::ToolApproval` ([`ui.rs:22`](../../crates/aictl-core/src/ui.rs)) is three values:
`Allow`, `Deny`, `AutoAccept`. `Allow` is one call; `AutoAccept` is everything, forever, in
this session. `/behavior` ([`commands/behavior.rs`](../../crates/aictl-cli/src/commands/behavior.rs))
offers the same binary: human-in-the-loop or auto.

So a user has two options: confirm every `read_file` in a fifty-call session, or hand the
agent unrestricted `exec_shell` and `remove_file`. There is nothing between, and the
practical result is that everyone flips to auto within about three turns — which means the
confirmation prompt, the codebase's main human-in-the-loop safety mechanism, is used for
about ninety seconds per session and then disabled.

The security policy has the right shape already: `security::ShellPolicy`
([`security.rs:76`](../../crates/aictl-core/src/security.rs)) has `allowed_commands` /
`blocked_commands`, and `PathPolicy` has `allowed_paths` / `blocked_paths`. Those are
*hard* policy — a denied call fails. What is missing is *soft* policy: "don't ask me about
this again", accumulated by the user during a session, persisted for next time.

## Goals & Non-goals

**Goals**

- A rule store: patterns that auto-approve (or auto-deny) matching tool calls without a
  prompt.
- Rules created **from the confirmation prompt itself** — the moment a user is confirming
  `cargo test` for the fourth time is the moment to offer "always allow this".
- Pattern granularity that matches how people actually think: by tool, by shell command
  prefix, by path glob.
- Project-scoped and user-scoped rule sets, with project rules winning.
- Rules are advisory-layer only: they can never widen what `security::validate_tool` permits.
- A management surface (`/approvals`) and honest visibility of what is auto-approved.

**Non-goals**

- No replacement for the security policy. Rules live *above* the gate, never around it.
- No rule import/sharing, no remote catalogue.
- No regex. Globs only — regex in a security-adjacent matcher is a footgun and the existing
  hook matcher already established globs as the house pattern.
- No time-limited rules at v1.
- No LLM-suggested rules.
- No server changes.

## Design

### 1. Rule model

```rust
// crates/aictl-core/src/approvals.rs
#[derive(Serialize, Deserialize, Clone)]
pub struct ApprovalRule {
    /// Tool name, or a glob over tool names (`mcp__github__*`).
    pub tool: String,
    /// Optional match against the tool body. Semantics depend on `kind`.
    pub pattern: Option<String>,
    pub kind: MatchKind,
    pub effect: Effect,          // Allow | Deny
    pub scope: Scope,            // User | Project
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum MatchKind {
    /// Any call to this tool.
    Any,
    /// The body's first line starts with `pattern` (shell command prefix).
    CommandPrefix,
    /// The tool's target path matches the glob in `pattern`.
    PathGlob,
}
```

Three match kinds, because they cover what people actually want to express:

- `Any` — "never ask about `read_file`".
- `CommandPrefix` — "always allow `cargo test`", matching `cargo test`, `cargo test --lib`,
  `cargo test foo`, but not `cargo publish`. Prefix on the **first line** of the body,
  compared token-wise so `cargo test` does not match `cargo testify`.
- `PathGlob` — "always allow `edit_file` under `src/**`", using the same glob engine as the
  hooks matcher.

The target path for `PathGlob` comes from the shared extraction helper introduced by the
checkpointing spec (§9 there) — one implementation of "which path does this call touch",
used by the gate, by capture, and by rules.

### 2. Evaluation order

In `run::handle_tool_call` / `run_parallel_call`, before the approval prompt and **after**
`security::validate_tool`:

```
1. security::validate_tool          → hard gate. Denied here = denied. Rules cannot override.
2. PreToolUse hook                  → can block or pre-approve (unchanged, still wins).
3. approvals::evaluate(call)        → Deny  → denied, reason names the rule
                                    → Allow → dispatch without prompting
                                    → None  → fall through
4. *auto (AutoAccept)               → dispatch
5. ui.confirm_tool                  → prompt the user
```

Placing rules **after** the security gate is the load-bearing decision: a rule can only skip
a *prompt*, never a *check*. `--unrestricted` bypasses the gate but not the rules; hooks stay
above rules, matching their documented status as harness behavior.

Deny rules are evaluated before allow rules, and project scope before user scope, so the most
specific prohibition wins.

### 3. Creating rules from the prompt

The confirmation prompt gains options. Today it is a crossterm selector with Allow / Deny /
AutoAccept; it becomes context-aware:

```
  Run tool: exec_shell
  cargo test --lib

  ▸ allow once
    allow always: `cargo test` in this project
    allow always: `cargo test` everywhere
    allow always: any exec_shell in this project
    deny once
    deny always: `cargo test`
    auto-accept everything this session
```

The "always" options are generated from the call: a shell call offers a command prefix (first
two tokens, which is where `cargo test` / `npm run` / `git log` land); a file tool offers the
directory glob of its target (`src/**` for `src/run.rs`); an MCP tool offers the server glob
(`mcp__github__*`).

This is the whole feature. A rule store nobody populates is worthless, and asking users to
hand-write globs in a config file guarantees nobody populates it. The prompt is where the
information ("I am tired of confirming this") and the intent both exist.

Selecting an "always" option writes the rule immediately and dispatches the call.

### 4. Storage

- **User scope**: `~/.aictl/approvals.json`.
- **Project scope**: `<local_config_root>/approvals.json` — i.e. `<cwd>/.aictl/approvals.json`,
  reusing `config::local_config_root()` so the `.aictl/` > `.claude/` precedence matches
  agents and skills.

Both are plain JSON arrays of `ApprovalRule`. Project rules are checked into the repo if the
user wants team-wide grants; that is a feature, and it is also why project rules must never
widen the security gate — a checked-in rules file from an untrusted repo must be incapable of
granting anything.

**Untrusted-repo consideration**: a project `approvals.json` arriving with a cloned repo could
auto-approve `exec_shell` on first run. Mitigation: project-scope rules are **inert until the
user confirms the file once**. On first sight of a project rules file, the CLI prints a
summary and asks; the answer is recorded (keyed by a hash of the file) in the user-scope
store. A changed file re-prompts. This mirrors how editors handle project-local settings and
is not optional.

### 5. Session rules

The prompt's "allow always … this session" variants write to an in-memory store that is never
persisted. Useful for the case the user does not want to commit to. `AutoAccept` remains
exactly as it is — the blunt instrument stays available.

### 6. Management surface

`/approvals`:

```
  project rules (.aictl/approvals.json)
    ✓ exec_shell    prefix `cargo test`
    ✓ edit_file     path `src/**`
    ✗ exec_shell    prefix `git push`         (deny)

  user rules (~/.aictl/approvals.json)
    ✓ read_file     any
    ✓ list_directory any

  session rules
    ✓ exec_shell    prefix `npm run`

  [a] add   [d] delete   [c] clear session   [C] clear all
```

`--list-approvals` prints the same non-interactively. `--no-approvals` disables the rule layer
for one launch (everything prompts again) — the "I want to watch it closely today" switch.

### 7. Visibility

An auto-approved call must not be invisible. When a rule fires, the existing
`AgentUI::show_auto_tool` path renders it with the rule that matched:

```
  ⚙ exec_shell  cargo test --lib          [rule: prefix `cargo test`]
```

The turn summary gains a count: `12 tool calls (7 auto-approved by rules)`. A user should
always be able to answer "what did it do without asking me?" — and after this spec, the
honest answer is usually "a lot", so the reporting has to be good.

### 8. Configuration

| Key | Default | Meaning |
|-----|---------|---------|
| `AICTL_APPROVALS_ENABLED` | `true` | Master switch for the rule layer |
| `AICTL_APPROVALS_PROJECT` | `true` | Honor project-scope rules at all |
| `AICTL_APPROVALS_FILE` | unset | Override the user-scope path (tests, sandboxes) |

### 9. Integration points

| File | Change |
|------|--------|
| `crates/aictl-core/src/approvals.rs` | **New** — rule model, glob matching, evaluation, both stores, project-trust prompt state |
| `crates/aictl-core/src/ui.rs` | Extend `ToolApproval` with rule-creating variants (see below) |
| `crates/aictl-core/src/run.rs` | Rule evaluation step; rule attribution in `show_auto_tool`; summary counter |
| `crates/aictl-core/src/security.rs` | Share the target-path extraction helper |
| `crates/aictl-core/src/config.rs` | Three keys |
| `crates/aictl-cli/src/ui.rs` | Context-aware confirmation menu |
| `crates/aictl-cli/src/commands/approvals.rs` | **New** — `/approvals` |
| `crates/aictl-cli/src/main.rs` | `--list-approvals`, `--no-approvals`; project-trust prompt at startup |
| `crates/aictl-desktop/src/commands/` | Rule list + delete under Settings → Security (read/delete only at v1; creation happens at the prompt, which the desktop renders differently) |
| `docs/USAGE.md`, `docs/CONFIG.md`, `docs/ARCH.md`, `CLAUDE.md` | Document, including the project-trust model |

`ToolApproval` grows rather than being replaced, so existing frontends keep compiling:

```rust
pub enum ToolApproval {
    Allow,
    Deny,
    AutoAccept,
    /// Allow this call and persist a rule.
    AllowAndRemember(ApprovalRule),   // new
    /// Deny this call and persist a rule.
    DenyAndRemember(ApprovalRule),    // new
}
```

The engine handles the two new variants by writing the rule then proceeding; a frontend that
never returns them behaves exactly as today.

### 10. Testing

**Unit**

- `CommandPrefix`: `cargo test` matches `cargo test --lib` and `cargo test foo`; does not
  match `cargo testify`, `cargo publish`, or `echo cargo test`; token-wise, not substring.
- `PathGlob`: `src/**` matches `src/a/b.rs`, not `docs/x.md`; a path outside the CWD jail never
  matches anything (defense in depth — the gate already rejected it).
- `Any` matches every call to that tool and nothing else; tool-name globs (`mcp__github__*`)
  work.
- Precedence: deny beats allow; project beats user; a project deny beats a user allow.
- Evaluation order: a rule cannot approve a call the security gate rejected (table test over
  gate-denied calls with a permissive rule present — **the key security test**).
- `--unrestricted` + rules: gate bypassed, rules still applied.
- Store round-trip; malformed JSON produces a warning and an empty rule set, never a panic.
- Project-trust: an unconfirmed project file contributes no rules; confirming records the
  hash; a modified file goes back to unconfirmed.

**Integration**

- Mock-LLM with a `PathGlob` allow rule: the edit dispatches with no `confirm_tool` call, and
  `show_auto_tool` reports the matching rule.
- A deny rule produces a denial result the model can read, and the loop continues.
- `--no-approvals` restores prompting.
- `AllowAndRemember` from a stubbed UI writes the rule and the next matching call skips the
  prompt.

**Manual smoke**

1. Confirm `cargo test` once with "always in this project"; verify the second call is silent
   and `/approvals` lists it.
2. Clone a repo containing an `approvals.json` granting `exec_shell` any; verify the trust
   prompt fires and rules are inert until confirmed.
3. `/approvals` delete round-trip.

### 11. Rollout

1. **`approvals.rs` + evaluation wiring + user-scope store.** Rules can only be written by
   hand-editing the file; the evaluation path and all its security tests land here.
2. **Prompt-driven rule creation** — the two new `ToolApproval` variants and the
   context-aware menu. This is what makes it usable.
3. **Project scope + trust prompt + `/approvals` + desktop read/delete.**

PR 3's trust model is the piece that must not be rushed; it is deliberately last and
separable.

### 12. Verification

1. `cargo build --workspace`, `cargo lint`, `cargo test` clean.
2. `AICTL_APPROVALS_ENABLED=false` reproduces today's prompting exactly (regression gate).
3. The security-precedence test suite passes: no rule, in any scope, permits a call the gate
   rejects.
4. Manual smoke checklist including the untrusted-repo case.
5. Turn summary reports auto-approved counts accurately across a mixed session.
6. CI gates:
   ```bash
   grep -rE 'approvals::' crates/aictl-server/src/     # must be empty
   # rules are evaluated in exactly one place
   grep -rn 'approvals::evaluate' crates/aictl-core/src/ | grep -v 'run.rs'   # must be empty
   ```

### 13. Risks

- **Rules becoming a shadow security policy.** Users will treat "always allow" as a security
  decision, and a bug that lets a rule widen the gate is a privilege escalation. Mitigation:
  the ordering in §2, the dedicated precedence test suite, the single-call-site CI gate, and
  docs that state plainly that rules skip prompts and nothing else.
- **Untrusted project rules.** Covered by the trust prompt (§4), but it is the highest-risk
  surface in this spec and the reason project scope lands last.
- **Over-broad grants accumulating.** Six months of "always allow" clicks produce a config
  nobody remembers agreeing to. Mitigation: `/approvals` shows everything; the turn summary
  counts auto-approvals; consider a periodic reminder (Open questions).
- **Prompt-menu bloat.** Seven options where there were three is more to read at the exact
  moment the user wants to move fast. Mitigation: "allow once" stays first and default;
  "always" options are generated only when meaningful (no path glob offered for a tool with
  no path); a `--simple-confirm` config restores the three-option menu.
- **Prefix matching too coarse.** `git` as a prefix would allow `git push --force`. Mitigation:
  the generated suggestion uses two tokens (`git log`, not `git`), and single-token prefixes
  for known-dangerous commands are refused with a note.
- **Interaction with parallel batches.** Phase 4 bundles approval for a batch. With rules, a
  batch may be partly rule-approved and partly not. Mitigation: evaluate rules per call first,
  then prompt once for whatever remains — strictly fewer prompts than today, never more.

### 14. Open questions

- **Should rules expire?** A 90-day TTL would bound accumulation, at the cost of re-prompting
  for things the user already decided. Lean no at v1; revisit if `/approvals` lists get long
  in practice.
- **Should the deny direction exist at all?** The security policy's `blocked_commands` already
  covers hard denials, so `Effect::Deny` is arguably redundant. Kept because a soft deny is
  useful ("don't let it touch `git push`, but I could override") and it costs almost nothing.
  Revisit if it goes unused.
- **Desktop rule creation.** The desktop's confirmation UI is a different component; giving it
  the same "always" affordances is a follow-up. Until then desktop users can only read and
  delete rules created in the CLI, which is a real asymmetry worth closing soon.
- **Should a rule ever auto-suggest itself** after N identical confirmations ("you have
  approved `cargo test` 5 times — always allow?")? Good UX, mild nag risk. Probably yes, as a
  follow-up, with a high N.
