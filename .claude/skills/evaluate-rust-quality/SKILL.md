---
name: evaluate-rust-quality
description: Evaluate project quality and Rust software development practices
allowed-tools: Bash, Read, Write, Glob, Grep
---

## Purpose

Audit the codebase for quality, idiomatic Rust usage, and software engineering best practices. Produce a concise report with findings and actionable recommendations.

## Workflow

### 1. Run automated checks

Run each of these commands via the Bash tool and capture the output:

    cargo clippy -- -W clippy::all -W clippy::pedantic 2>&1

    cargo test 2>&1

    cargo fmt --check 2>&1

    cargo build 2>&1

Record any warnings, errors, or failures for the report.

### 2. Inspect project structure

Read Cargo.toml for:

- Rust edition -- should be 2021 or later
- Dependencies -- check for unusually old versions or redundant crates
- Missing recommended fields (description, repository, license)

Use Glob to list all src/**/*.rs files. Verify the module layout is logical and each file has a clear, single responsibility.

### 3. Review error handling

Use the Grep tool to search the codebase for anti-patterns:

- .unwrap() -- flag uses outside of tests and infallible cases (e.g. regex compilation, known-valid data). Each call should be justified or replaced with proper error handling.
- .expect() -- acceptable if the message explains the invariant, but prefer ? propagation.
- panic! / unreachable! / todo! -- flag any in non-test code.
- Bare Box<dyn std::error::Error> -- note if a custom error type would improve the API.

### 4. Review safety and security

Use the Grep tool to search for:

- unsafe blocks -- flag and verify each is necessary and documented.
- Command injection risk -- check that user-provided strings passed to shell commands are properly handled.
- Hardcoded secrets or credentials -- no API keys, tokens, or passwords in source.
- File path handling -- verify no path traversal vulnerabilities.

### 5. Review code quality

Use the Grep and Read tools to scan for common Rust quality issues:

- Cloning -- search for .clone() and flag unnecessary clones where a borrow would suffice.
- String handling -- check for excessive String allocation where &str would work.
- Dead code -- look for #[allow(dead_code)] or unused imports/functions.
- Magic numbers -- flag numeric literals that should be named constants.
- Function length -- flag functions longer than roughly 80 lines that could be decomposed.
- Public API surface -- check that only items meant for external use are pub.

### 6. Review testing

- Use Grep to count #[test] functions and note the total.
- Check which modules have tests and which lack coverage.
- Look for test quality: do tests assert behavior (not just absence of panic)?
- Check for integration tests in tests/ directory.

### 7. Review documentation

- Check for module-level doc comments (//!) in each source file.
- Check for doc comments (///) on public functions and types.
- Verify README.md exists and covers installation, usage, and configuration.

### 8. Produce the report

Print a structured report with these sections:

    ## Automated Checks
    pass/fail summary for clippy, tests, fmt, build

    ## Project Structure
    module layout assessment, Cargo.toml observations

    ## Error Handling
    findings with file:line references

    ## Safety & Security
    findings with file:line references

    ## Code Quality
    findings with file:line references

    ## Testing
    test count, coverage gaps, quality notes

    ## Documentation
    doc comment coverage, README status

    ## Summary
    overall assessment: score out of 10, top 3 strengths, top 3 improvements

### 9. Save the report

After printing the report, save it to the .claude/reports/ directory:

- Use the Bash tool to get the current timestamp: date '+%Y-%m-%d_%H-%M-%S'
- Write the report as a markdown file named rust-quality-report-YYYY-MM-DD_HH-MM-SS.md
- The file path is .claude/reports/rust-quality-report-<timestamp>.md
- Add a top-level heading with the date and time: # Evaluation Report -- YYYY-MM-DD HH:MM:SS
- Confirm the file was saved by printing the path.

## Rules

- Reference specific files and line numbers for every finding.
- Distinguish between issues (should fix) and suggestions (nice to have).
- Do not modify any code -- this skill is read-only analysis.
- Do not report on generated files, build artifacts, or vendored dependencies.
- Keep the report concise -- one line per finding, grouped by section.
- Be objective -- note strengths as well as problems.
