---
name: create-hook
description: Add a lifecycle hook to ~/.aictl/hooks.json. Walks the user through choosing the right event, matcher, command, and timeout, then merges the new entry into the existing config without disturbing other hooks.
allowed-tools: Bash, Read, Edit, Write
---

## Purpose

Help the user add a new hook to their personal aictl hooks file at `~/.aictl/hooks.json`. The aictl harness fires hooks at lifecycle events (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `PreCompact`, `Notification`, `SessionEnd`) and lets each hook influence the harness via JSON returned on stdout. This skill captures intent, picks the right event, drafts a safe shell command, and edits the file in place.

Source of truth for hook semantics: `src/hooks.rs` in this repo. When in doubt, read it before answering — the parser, matcher, and decision shapes are authoritative there.

## Inputs to gather from the user

Before editing the file, get clear answers to all four:

1. **When** should the hook fire? (Pick exactly one event.)
2. **Which targets** should it match? (Tool name(s) for tool events, `*` for everything else.)
3. **What** should it do? (The shell command.)
4. **How should it influence the agent** — observe only, block, approve, add context, or rewrite the prompt?

Default `timeout` to `60` (seconds) unless the command is known-fast or known-slow. Default `enabled` to `true` unless the user is staging the hook for later.

## Event reference

Pick the most specific event that fits. Wrong event = hook fires at the wrong time.

| Event | Fires when | Common use |
|-------|-----------|------------|
| `SessionStart` | REPL boots; `--message` single-shot starts | logging, env warmup |
| `SessionEnd` | REPL exits; single-shot finishes | cleanup, transcript export |
| `UserPromptSubmit` | After the user hits Enter, **before** the injection guard | block credentialed prompts; rewrite prompts |
| `PreToolUse` | Before a tool runs (and before the user y/N prompt) | deny dangerous tool calls; pre-approve safe ones |
| `PostToolUse` | After a tool produces output; result is already in history | auto-format, lint, append "tests passed" context |
| `Stop` | After the agent's final answer (no tool call) | log, snapshot, notify |
| `PreCompact` | Before `/compact` summarizes the conversation | back up the raw transcript |
| `Notification` | Inside the `notify` tool, before the OS pop | mirror to webhook; suppress noisy alerts |

## Matcher

A glob over the tool name (or `*` for non-tool events).

- `*` — matches anything (only meaningful pattern for non-tool events).
- `exec_shell` — exact match.
- `read_*` — prefix.
- `edit_file|write_file` — alternation; either matches.
- `mcp__*__*` — wildcards anywhere; useful for namespaced MCP tools.
- `?` matches a single character.

For a non-tool event the matcher is conventionally `"*"` (the harness sends an empty match target — only `*` and empty patterns hit).

## Stdin payload

The hook receives one JSON object on stdin. Fields present depend on the event:

```json
{
  "event": "PreToolUse",
  "session_id": "uuid-or-absent-in-single-shot",
  "cwd": "/Users/you/project",
  "tool": { "name": "exec_shell", "input": "ls -la" },
  "prompt": "...",
  "notification": "...",
  "trigger": "startup|resume|repl-exit|single-shot|auto|manual|test",
  "matcher": "exec_shell"
}
```

Only `event` is always present. `tool` appears for `PreToolUse` / `PostToolUse`. `prompt` appears for `UserPromptSubmit` / `Stop`. `notification` for `Notification`. `trigger` for the lifecycle events that have a meaningful flavor.

## Stdout — how the hook influences the harness

Five shapes. Empty stdout means "continue, do nothing".

| Stdout | Effect |
|--------|--------|
| `""` (empty) | Continue silently. |
| `{"decision":"block","reason":"..."}` | Abort the action; reason is surfaced to the LLM as the tool result (or rejection message for prompt events). |
| `{"decision":"approve","reason":"..."}` | Pre-approve a tool call — skip the user's y/N prompt. Stays human-in-the-loop friendly. |
| `{"additionalContext":"..."}` | Inject a `<hook_context>` user turn into history before the next LLM call. |
| `{"rewrittenPrompt":"..."}` | `UserPromptSubmit` only — replace the user's text before the agent sees it. |
| `<plain text>` | Treated as `additionalContext`. |

Shorthand: **exit code 2** is equivalent to `{"decision":"block","reason":"<stderr>"}`. Useful for one-line shell hooks.

Hooks run via `sh -c` in the security working directory with a scrubbed env. Default timeout is 60s; failures (spawn error, timeout, non-2 nonzero exit) are logged to stderr and treated as Continue so a broken hook can't wedge the agent loop.

## Workflow

### 1. Confirm the file path

The default is `~/.aictl/hooks.json`. If `AICTL_HOOKS_FILE` is set in `~/.aictl/config`, use that instead. Check both before editing:

```sh
grep -E '^[[:space:]]*AICTL_HOOKS_FILE' ~/.aictl/config 2>/dev/null
```

### 2. Read the current file

```sh
cat ~/.aictl/hooks.json 2>/dev/null
```

If the file is missing, empty, or contains only `{}`, treat the starting state as `{}`. If JSON parsing fails, stop and ask the user — do not overwrite a malformed file blindly.

### 3. Draft the new entry

Build a single JSON object:

```json
{
  "matcher": "<glob>",
  "command": "<shell command>",
  "timeout": 60,
  "enabled": true
}
```

Drop `enabled` when it's `true` (the default). Keep `timeout` explicit so the hook is self-documenting.

If the user wants comments, use `_comment` keys (top-level or per-entry) — the parser silently skips underscore-prefixed top-level keys; per-entry unknown keys are ignored.

### 4. Merge into the existing file

The hooks file maps event names to **arrays** of hooks. Append the new entry to the array for the chosen event; create the array if it doesn't exist; keep every other event untouched.

Use `Read` to load the current file, then `Edit` or `Write` to produce the merged result. Pretty-print with 2-space indent so diffs stay readable.

Do **not** clobber the file with `Write` unless you've first read its full content and reconstructed every existing entry. If unsure, pipe through `jq`:

```sh
jq '.<Event> += [<NEW_ENTRY>]' ~/.aictl/hooks.json > /tmp/hooks.json && mv /tmp/hooks.json ~/.aictl/hooks.json
```

### 5. Validate

After saving, run:

```sh
aictl --list-hooks
```

Confirm the new entry appears under the right event with the expected matcher/command/timeout. If the file failed to parse, aictl prints `hooks: failed to parse <path>: <reason>` to stderr — re-read and fix.

### 6. Suggest test-firing

Tell the user they can test the hook end-to-end without running a real turn:

- Start `aictl`, type `/hooks`, choose **test-fire a hook**, pick the new entry.
- The skill harness sends a synthetic payload matching the event kind, runs the command, and prints the parsed decision (block / approve / additionalContext / continue).

For a quick non-REPL smoke test, ask `aictl` a question and watch the relevant side effect (a tail of the log file, the suppressed shell command, etc.).

## Rules

- Always read `~/.aictl/hooks.json` before writing. Preserve every existing entry.
- Never wrap the command in quoting that the user's shell will mangle — keep the JSON-escaped form clean.
- Use the most specific matcher that covers the user's intent. `*` on a tool event fires for every tool, including ones the user didn't ask about.
- For dangerous decisions (block, rewrite prompt) confirm the wording of the `reason` / `rewrittenPrompt` with the user before saving.
- Default timeout: 60s. Lower it (5–15s) for fast hooks (`echo`, `printf`, file appends). Raise it (120–300s) only when the command is genuinely slow (formatters on large repos).
- Set `enabled: false` if the user wants the hook staged but inert — they can flip it on later via `/hooks → toggle a hook`.
- Do not commit or push the user's `~/.aictl/hooks.json` — it is personal config, not project state.
- Do not invent fields. The parser accepts only `matcher`, `command`, `timeout`, `enabled` per entry, plus underscore-prefixed comment keys at the top level.

## Canonical examples

Pick whichever shape matches the user's intent and adapt the matcher / command.

### Log every completed turn (Stop, observe-only)

```json
{
  "Stop": [
    {
      "matcher": "*",
      "command": "date '+turn ended at %H:%M:%S' >> /tmp/aictl-hook.log",
      "timeout": 5
    }
  ]
}
```

### Block `git push` from `exec_shell` (PreToolUse, deny via exit 2)

```json
{
  "PreToolUse": [
    {
      "matcher": "exec_shell",
      "command": "jq -r '.tool.input' | grep -Eq '^[[:space:]]*git[[:space:]]+push' && (echo 'no git push from hooks' >&2; exit 2) || true",
      "timeout": 10
    }
  ]
}
```

### Auto-format Rust after every edit (PostToolUse, additionalContext)

```json
{
  "PostToolUse": [
    {
      "matcher": "edit_file|write_file",
      "command": "cargo fmt --message-format short 2>&1 | head -c 2000",
      "timeout": 30
    }
  ]
}
```

### Refuse credential-shaped prompts (UserPromptSubmit, block)

```json
{
  "UserPromptSubmit": [
    {
      "matcher": "*",
      "command": "jq -e 'select(.prompt | test(\"AKIA[0-9A-Z]{16}|sk-[A-Za-z0-9]{20,}\"))' >/dev/null && printf '{\"decision\":\"block\",\"reason\":\"prompt looks like it contains credentials\"}' || true",
      "timeout": 5
    }
  ]
}
```

### Pre-approve all `read_file` calls in human-in-the-loop mode (PreToolUse, approve)

```json
{
  "PreToolUse": [
    {
      "matcher": "read_file",
      "command": "printf '{\"decision\":\"approve\",\"reason\":\"reads are always allowed\"}'",
      "timeout": 5
    }
  ]
}
```

## Report back

After saving, tell the user:

- The exact path of the file you edited.
- The event, matcher, and command you added (one line each).
- Whether `aictl --list-hooks` confirmed the parse.
- One concrete way to verify it (the side effect to watch for, or `/hooks → test-fire` if test-firing is appropriate).

Do not run `aictl` itself — only `aictl --list-hooks`. Anything beyond a list invokes a real provider and is the user's call.
