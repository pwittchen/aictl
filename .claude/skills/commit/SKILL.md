---
name: commit
description: Commit staged and unstaged changes with a clear commit message
allowed-tools: Bash
---

## Commit workflow

1. Run `git status` to see all changed and untracked files.
2. Run `git diff` to review both staged and unstaged changes.
3. Run `git log --oneline -5` to see recent commit style.
4. Stage relevant files by name — avoid `git add -A` or `git add .`.
5. Write a commit message following the rules below and commit.
6. Run `git status` to verify success.

## Commit message rules

- NEVER add `Co-Authored-By`, `Signed-off-by`, or any AI attribution lines.
- Small, trivial changes (typo fix, one-liner, rename) get a single short line — no body needed.
- Larger changes get a short summary line followed by a blank line and a detailed body explaining what changed and why.
- Summary line must be imperative mood, max 72 characters (e.g. "Add OpenAI provider support").
- Body lines wrap at 72 characters.
- Focus on **why** the change was made, not just **what** changed.
- Do not repeat filenames or diffs in the message — describe intent.

## Examples

Small change:
```
Fix typo in README
```

Larger change:
```
Add streaming support for Anthropic provider

Send requests with stream=true and read server-sent events
incrementally, printing tokens as they arrive. This gives immediate
feedback for long responses instead of waiting for the full completion.
```
