---
name: project-stats-report
description: Generate a general project statistics report (LOC, commit activity, contributors, etc.) and save it under .claude/reports/stats/ with a timestamp.
allowed-tools: Bash, Read, Write, Glob, Grep
---

## Purpose

Produce a snapshot of overall project health and activity metrics so the user can track how the project evolves over time. Each run is saved as a timestamped markdown file under `.claude/reports/stats/`, building up a history of comparable reports.

## Workflow

### 1. Collect repository activity

Run via the Bash tool and capture output:

    git rev-list --count HEAD
    git log --reverse --format=%aI | head -n 1
    git log -1 --format=%aI
    git log --format=%aI | awk -F'T' '{print $1}' | sort -u | wc -l
    git shortlog -sne --all
    git log --since="30 days ago" --oneline | wc -l
    git log --since="7 days ago" --oneline | wc -l
    git log --format=%aI | awk -F'T' '{print substr($1,1,7)}' | sort | uniq -c | tail -n 12
    git branch -a | wc -l
    git tag | wc -l
    git log -1 --format='%h %s (%aI, %an)'

Derive:

- Total commits
- First commit date, latest commit date
- Days of active development (unique commit days) and total elapsed days
- Commits in the last 7 / 30 days
- Commits per month (last 12 months)
- Contributors with commit counts
- Branch and tag counts

### 2. Collect lines-of-code stats

Prefer `tokei` if available; otherwise fall back to `cloc`; otherwise compute manually.

    command -v tokei && tokei --output json
    command -v cloc && cloc . --json --quiet --exclude-dir=target,node_modules,.git

If neither is installed, fall back via Bash + Glob:

    git ls-files | xargs wc -l 2>/dev/null | tail -n 1

Group reported LOC by language when possible (code / comments / blanks). Report total source files.

### 3. Collect project structure stats

- Use Glob `src/**/*.rs` (or the appropriate pattern for the detected language) to count source files and modules.
- Read `Cargo.toml` (or equivalent manifest) for: project name, version, edition, dependency count (split runtime vs dev), declared features.
- Count `#[test]` functions via Grep for a quick test surface number.
- Count TODO/FIXME/HACK/XXX markers via Grep.
- Note the largest source files (top 5 by line count).

### 4. Collect repository size

    du -sh .git 2>/dev/null
    du -sh --exclude=target --exclude=.git . 2>/dev/null

Report repo on-disk size excluding build artifacts, plus `.git` size separately.

### 5. Assemble the report

Structure the markdown report:

    # Project Stats Report -- YYYY-MM-DD HH:MM:SS

    ## Overview
    name, version, current branch, latest commit (hash + subject + date)

    ## Commit Activity
    total commits, first/last commit dates, elapsed days, active development days,
    commits last 7d / 30d, commits per month (last 12), branches, tags

    ## Contributors
    table or list: name <email> -- N commits

    ## Lines of Code
    per-language breakdown (files, code, comments, blanks), totals,
    tool used (tokei / cloc / wc fallback)

    ## Project Structure
    source files, modules, dependency counts, feature flags,
    test count, TODO/FIXME count, top 5 largest files

    ## Repository Size
    working tree size (excluding target/ and .git/), .git size

    ## Notes
    anything noteworthy compared to typical project shape (only if obvious from data;
    do not invent trends -- comparison to prior reports is out of scope for a single run)

### 6. Save the report

- Get the timestamp via Bash: `date '+%Y-%m-%d_%H-%M-%S'`
- Ensure the directory exists: `mkdir -p .claude/reports/stats`
- Write the report to `.claude/reports/stats/project-stats-report-<timestamp>.md`
- Print the saved file path to confirm.

## Rules

- Read-only: never modify project source, configuration, or git state.
- Skip `target/`, `node_modules/`, `.git/`, and other build/vendor directories when measuring code.
- If a command is unavailable (e.g. `tokei`, `cloc`), fall back gracefully and note the tool used in the report.
- Keep the report factual -- numbers and short labels, no speculation about quality or velocity.
- Do not delete or overwrite previous reports; each run produces a new timestamped file so history accumulates.
- Use absolute, unambiguous numbers; avoid relative phrases like "recently" without dates.
