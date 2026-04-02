---
name: evaluate-rust-security
description: Evaluate project security posture with focus on Rust and CLI-specific risks
allowed-tools: Bash, Read, Glob, Grep, Write
---

## Purpose

Audit the codebase for security vulnerabilities, unsafe patterns, and CLI-specific risks. Produce a concise report with findings, severity ratings, and actionable recommendations.

## Workflow

### 1. Run cargo audit and clippy security lints

Run each command via the Bash tool and capture output:

    cargo audit 2>&1 || true

    cargo clippy -- -W clippy::all -W clippy::pedantic -W clippy::correctness 2>&1

If `cargo-audit` is not installed, note this as a finding and skip.

### 2. Review unsafe code

Use Grep to search for all `unsafe` blocks and functions:

- Flag every `unsafe` block -- verify each is necessary, minimal in scope, and has a SAFETY comment.
- Check for raw pointer dereferences, transmute, and FFI calls.
- Verify unsafe abstractions expose a safe public API.

### 3. Review command execution and injection risks

Use Grep and Read to examine all shell/process execution:

- Search for `Command::new`, `sh -c`, `bash -c`, `tokio::process`, `std::process`.
- Check whether user-controlled input flows into command arguments without sanitization.
- Verify argument construction -- prefer passing args as separate arguments over string interpolation into a shell string.
- Check for environment variable injection -- does the tool inherit or control the subprocess environment?

### 4. Review input validation and parsing

Use Grep and Read to check CLI input handling:

- Search for clap argument definitions -- verify validators and value parsers are used where appropriate.
- Check file path arguments for path traversal risks (e.g. `../`, symlink following).
- Check numeric inputs for overflow or out-of-range values.
- Verify REPL input is bounded -- no unbounded reads that could exhaust memory.
- Check for format string injection in any logging or output formatting.

### 5. Review network and API security

Use Grep and Read to examine HTTP and API usage:

- Search for `reqwest`, `hyper`, `HTTP`, `https`, `fetch`, `url`.
- Verify TLS is enforced -- no plain HTTP for sensitive endpoints.
- Check that API keys and tokens are read from config, not hardcoded.
- Verify API responses are validated before use -- no blind trust of remote JSON.
- Check for SSRF risks -- can a user cause the tool to make requests to arbitrary URLs?
- Check timeout settings -- requests without timeouts can hang indefinitely.

### 6. Review secrets and credential handling

Use Grep and Read to check for secret exposure:

- Search for patterns: `api_key`, `token`, `secret`, `password`, `credential`, `auth`.
- Verify secrets are read from config files, not embedded in source.
- Check that secrets are not logged, printed, or included in error messages.
- Verify config file permissions are documented (should be user-readable only).
- Check .gitignore for config files containing secrets.

### 7. Review file system operations

Use Grep and Read to examine file handling:

- Search for `fs::read`, `fs::write`, `fs::remove`, `fs::create_dir`, `File::open`, `File::create`.
- Check for TOCTOU (time-of-check-time-of-use) races in file operations.
- Verify file writes use atomic operations or temp files where data integrity matters.
- Check for symlink following risks -- does the tool follow symlinks into unexpected locations?
- Verify file permissions are set appropriately on created files.

### 8. Review dependency supply chain

Read Cargo.toml and Cargo.lock:

- Count total dependencies (direct and transitive).
- Flag dependencies with very few downloads or unknown maintainers if cargo-audit data is available.
- Check for wildcard version requirements (e.g. `*` or overly broad ranges).
- Verify lock file is committed to version control.

### 9. Review error handling for information leaks

Use Grep and Read to check error paths:

- Search for error display and formatting -- verify internal details (stack traces, file paths, memory addresses) are not exposed to users in release mode.
- Check that panics in production code are minimized -- search for `unwrap()`, `expect()`, `panic!` outside tests.
- Verify error messages do not leak secrets, tokens, or sensitive config values.

### 10. Review denial of service risks

Use Grep and Read to check for resource exhaustion:

- Check for unbounded allocations -- `Vec` or `String` growing from untrusted input without limits.
- Check for unbounded loops or recursion driven by external input.
- Verify timeouts on all blocking operations (network, process execution, file reads).
- Check output size -- can a tool result flood the terminal or exhaust memory?

### 11. Produce the report

Print a structured report with these sections:

    ## Dependency Audit
    cargo-audit results, supply chain observations

    ## Unsafe Code
    findings with file:line references and severity

    ## Command Execution
    injection risks with file:line references and severity

    ## Input Validation
    CLI and REPL input handling findings

    ## Network Security
    TLS, SSRF, timeout, response validation findings

    ## Secrets Management
    credential handling findings

    ## File System Security
    file operation findings

    ## Error Handling & Info Leaks
    error path findings

    ## Denial of Service
    resource exhaustion findings

    ## Summary
    overall security posture: score out of 10,
    critical issues (must fix), warnings (should fix), suggestions (nice to have)

Use severity labels for each finding: CRITICAL, HIGH, MEDIUM, LOW, INFO.

### 12. Save the report

After printing the report, save it to the .claude/reports/ directory:

- Use the Bash tool to get the current timestamp: date '+%Y-%m-%d_%H-%M-%S'
- Write the report as a markdown file named rust-security-report-YYYY-MM-DD_HH-MM-SS.md
- The file path is .claude/reports/rust-security-report-<timestamp>.md
- Add a top-level heading with the date and time: # Security Report -- YYYY-MM-DD HH:MM:SS
- Confirm the file was saved by printing the path.

## Rules

- Reference specific files and line numbers for every finding.
- Assign a severity to every finding: CRITICAL, HIGH, MEDIUM, LOW, or INFO.
- Do not modify any code -- this skill is read-only analysis.
- Do not report on generated files, build artifacts, or vendored dependencies.
- Keep the report concise -- one line per finding, grouped by section.
- Be objective -- note strengths as well as problems.
- Focus on real, exploitable risks over theoretical concerns.
