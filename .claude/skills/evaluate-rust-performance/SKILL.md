---
name: evaluate-rust-performance
description: Evaluate project performance with focus on Rust and CLI-specific patterns
allowed-tools: Bash, Read, Glob, Grep, Write
---

## Purpose

Audit the codebase for performance issues, inefficient patterns, and CLI responsiveness problems. Produce a concise report with findings and actionable recommendations.

## Workflow

### 1. Build and measure baseline

Run each command via the Bash tool and capture output:

    cargo build --release 2>&1

    ls -lh target/release/$(basename $(pwd)) 2>/dev/null || ls -lh target/release/*.exe 2>/dev/null || echo "binary not found"

    time cargo build --release 2>&1

Record binary size and build time.

### 2. Review allocations and cloning

Use Grep and Read to find allocation-heavy patterns:

- Search for `.clone()` -- flag clones in hot paths or loops where a borrow would suffice.
- Search for `.to_string()`, `.to_owned()` -- flag unnecessary conversions from &str to String.
- Search for `format!` in loops -- flag repeated allocations that could be pre-allocated.
- Search for `String::new()` followed by repeated `push_str` -- suggest using `format!` or pre-allocated capacity.
- Search for `Vec::new()` in loops without `with_capacity` -- flag when the size is known or estimable.
- Search for `collect()` into intermediate collections that are immediately iterated again.

### 3. Review string handling

Use Grep and Read to examine string usage:

- Flag functions accepting `String` parameters where `&str` would work.
- Flag functions returning `String` where the caller always borrows the result.
- Search for repeated string concatenation with `+` operator -- suggest `format!` or `push_str`.
- Check for unnecessary `String::from` or `.into()` conversions at call sites.

### 4. Review async and concurrency

Use Grep and Read to examine async patterns:

- Search for `tokio::spawn`, `.await`, `async fn` -- verify async is used appropriately.
- Flag sequential `.await` calls that could run concurrently with `join!` or `try_join!`.
- Search for blocking operations inside async contexts -- `std::fs`, `std::thread::sleep`, heavy computation without `spawn_blocking`.
- Check for unnecessary `Arc`/`Mutex` where simpler ownership would work.
- Flag `tokio::sync::Mutex` vs `std::sync::Mutex` misuse.

### 5. Review I/O and network performance

Use Grep and Read to examine I/O patterns:

- Search for `reqwest` usage -- check for connection reuse (shared `Client` vs per-request `Client::new()`).
- Check for missing timeouts on HTTP requests and process execution.
- Search for unbuffered file reads -- `read_to_string` on large files without size checks.
- Flag reading entire files when only a portion is needed.
- Check for sequential HTTP requests that could be parallelized.

### 6. Review process execution

Use Grep and Read to examine subprocess handling:

- Search for `Command::new`, `tokio::process::Command` -- check for efficient usage.
- Flag commands that capture stdout/stderr unnecessarily.
- Check for missing output size limits on subprocess output.
- Verify process timeouts are in place to prevent hangs.

### 7. Review data structures and algorithms

Use Grep and Read to check for inefficient patterns:

- Search for linear scans (`iter().find`, `iter().position`, `contains`) on large collections -- suggest HashMap/HashSet.
- Flag repeated lookups in the same collection within a function.
- Check for unnecessary sorting or repeated sorting of the same data.
- Search for `regex::Regex::new` inside loops -- should be compiled once and reused.
- Flag deeply nested loops (3+ levels) that could indicate algorithmic issues.

### 8. Review binary size contributors

Use Grep and Read to check for bloat:

- Read Cargo.toml -- check for heavy dependencies that could be replaced with lighter alternatives.
- Check for unused feature flags on dependencies.
- Search for `#[derive(Debug)]` on large types that don't need it in release builds.
- Check if LTO (link-time optimization) is configured in release profile.
- Check if `strip` is configured in release profile for smaller binaries.

### 9. Review startup and CLI responsiveness

Use Grep and Read to check startup performance:

- Check for expensive initialization at startup -- large file reads, network calls, heavy parsing.
- Verify lazy initialization is used where appropriate (OnceLock, lazy_static).
- Check clap configuration -- verify derive mode isn't pulling in unnecessary overhead.
- Flag any blocking operations before the first user-visible output.

### 10. Produce the report

Print a structured report with these sections:

    ## Build Metrics
    binary size, build time, release profile settings

    ## Allocations & Cloning
    findings with file:line references

    ## String Handling
    findings with file:line references

    ## Async & Concurrency
    findings with file:line references

    ## I/O & Network
    findings with file:line references

    ## Process Execution
    findings with file:line references

    ## Data Structures & Algorithms
    findings with file:line references

    ## Binary Size
    dependency weight, feature flags, optimization settings

    ## Startup & Responsiveness
    findings with file:line references

    ## Summary
    overall performance assessment: score out of 10,
    critical issues (measurable impact), warnings (likely impact),
    suggestions (marginal improvement)

Use impact labels for each finding: CRITICAL, HIGH, MEDIUM, LOW, INFO.

### 11. Save the report

After printing the report, save it to the .claude/reports/ directory:

- Use the Bash tool to get the current timestamp: date '+%Y-%m-%d_%H-%M-%S'
- Write the report as a markdown file named rust-performance-report-YYYY-MM-DD_HH-MM-SS.md
- The file path is .claude/reports/rust-performance-report-<timestamp>.md
- Add a top-level heading with the date and time: # Performance Report -- YYYY-MM-DD HH:MM:SS
- Confirm the file was saved by printing the path.

## Rules

- Reference specific files and line numbers for every finding.
- Assign an impact label to every finding: CRITICAL, HIGH, MEDIUM, LOW, or INFO.
- Do not modify any code -- this skill is read-only analysis.
- Do not report on generated files, build artifacts, or vendored dependencies.
- Keep the report concise -- one line per finding, grouped by section.
- Be objective -- note strengths as well as problems.
- Focus on measurable or likely performance impact over micro-optimizations.
- Prefer findings that affect user-perceived latency and responsiveness.
