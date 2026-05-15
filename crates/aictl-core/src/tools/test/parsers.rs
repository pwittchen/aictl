//! Per-runner output parsers for the `test` tool.
//!
//! Each parser is best-effort: when the output is in a shape we
//! recognise, we extract pass/fail/skipped counts plus per-failure
//! details; otherwise we leave counts at zero and set `parse_warning`
//! so the agent loop can fall back to the raw tail.

use super::{TestFailure, TestSummary};

/// Parse `cargo test` output.
///
/// Looks for the `test result: ok. N passed; N failed; N ignored;` line
/// and, when failures are present, the `failures:` section that lists
/// the failing test names plus the `---- <name> stdout ----` blocks
/// that carry the assertion message.
pub(super) fn parse_cargo(stdout: &str, stderr: &str) -> TestSummary {
    let combined = combine(stdout, stderr);
    let mut s = TestSummary::default();

    let mut found_result_line = false;
    for line in combined.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("test result:") {
            // Lines look like:
            //   test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
            //   test result: FAILED. 2 passed; 1 failed; 0 ignored; …
            // The leading "ok." / "FAILED." chases off the helper's
            // "N <label>" scan, so trim the verdict prefix off first.
            // Multiple result lines may appear (one per test target) — sum.
            let payload = rest
                .trim_start()
                .strip_prefix("ok.")
                .or_else(|| rest.trim_start().strip_prefix("FAILED."))
                .unwrap_or(rest);
            parse_label_count_pairs(payload, &mut s);
            found_result_line = true;
        }
    }

    let failures = extract_cargo_failures(&combined);
    s.failures = failures;
    // Defensive: if the result line undercounts (rare) or is missing,
    // fall back to the parsed list length.
    if (s.failed as usize) < s.failures.len() {
        s.failed = s.failures.len().try_into().unwrap_or(u32::MAX);
    }

    if !found_result_line && s.failures.is_empty() {
        s.parse_warning =
            Some("cargo test output did not include a `test result:` line".to_string());
    }
    s
}

fn extract_cargo_failures(combined: &str) -> Vec<TestFailure> {
    // Names come from `---- <name> stdout ----` headers — one per
    // failing test, regardless of whether the corresponding bodies hit
    // stdout or stderr. The trailing `failures:` block with bare names
    // is a duplicate of this list, so we don't need it.
    let mut names: Vec<String> = Vec::new();
    for line in combined.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("---- ")
            && let Some(name) = rest.strip_suffix(" stdout ----")
        {
            let name = name.trim().to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }

    let mut failures: Vec<TestFailure> = names
        .into_iter()
        .map(|name| TestFailure {
            name,
            ..Default::default()
        })
        .collect();

    // Attach assertion messages by scanning the `---- <name> stdout ----`
    // blocks.
    for f in &mut failures {
        let needle = format!("---- {} stdout ----", f.name);
        if let Some(idx) = combined.find(&needle) {
            let after = &combined[idx + needle.len()..];
            let block_end = after.find("\n\n").unwrap_or(after.len());
            let body = after[..block_end].trim();
            // Pull the first `assertion failed:` / `panicked at` line for
            // a tight message; otherwise keep the first non-empty line.
            let mut msg: Option<String> = None;
            let mut loc: Option<String> = None;
            for line in body.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if msg.is_none()
                    && (line.starts_with("assertion ")
                        || line.starts_with("panicked at")
                        || line.starts_with("thread '"))
                {
                    msg = Some(line.to_string());
                }
                if loc.is_none()
                    && line.starts_with("at ")
                    && let Some(rest) = line.strip_prefix("at ")
                {
                    loc = Some(rest.to_string());
                }
            }
            if msg.is_none() {
                msg = body
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_string);
            }
            if let Some(m) = msg {
                f.message = m;
            }
            f.location = loc;
        }
    }

    failures
}

/// Parse output from a typical Node test runner (jest / vitest / mocha
/// via `npm test`). Best-effort across formats.
pub(super) fn parse_npm(stdout: &str, stderr: &str) -> TestSummary {
    let combined = combine(stdout, stderr);
    let mut s = TestSummary::default();

    // jest / vitest: "Tests:       3 passed, 1 failed, 1 skipped, 5 total"
    for line in combined.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Tests:") {
            parse_label_count_pairs(rest, &mut s);
        }
        // mocha summary often looks like:
        //   12 passing (1.2s)
        //   1 failing
        //   2 pending
        if trimmed.ends_with("passing") || trimmed.ends_with(" passing (1.2s)") {
            if let Some((n, _)) = split_first_number(trimmed) {
                s.passed += n;
            }
        } else if let Some((n, rest)) = split_first_number(trimmed) {
            if rest.starts_with("passing") {
                s.passed += n;
            } else if rest.starts_with("failing") {
                s.failed += n;
            } else if rest.starts_with("pending") || rest.starts_with("skipped") {
                s.skipped += n;
            }
        }
    }

    // jest failure lines: "  ● <test name>" or "FAIL path/to/file" — we
    // pick the `●` form because it's per-test, not per-file.
    let mut failures: Vec<TestFailure> = Vec::new();
    for (i, line) in combined.lines().enumerate() {
        if let Some(name) = line.trim_start().strip_prefix("● ") {
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let message = combined
                .lines()
                .skip(i + 1)
                .find(|l| {
                    let t = l.trim();
                    !t.is_empty() && !t.starts_with("●")
                })
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
            failures.push(TestFailure {
                name,
                message,
                location: None,
            });
        }
    }
    if (s.failed as usize) < failures.len() {
        s.failed = failures.len().try_into().unwrap_or(u32::MAX);
    }
    s.failures = failures;

    if s.passed == 0 && s.failed == 0 && s.skipped == 0 {
        s.parse_warning =
            Some("npm/node test output not recognised; counts unavailable".to_string());
    }

    s
}

/// Parse pytest output.
///
/// Recognises the summary line
///   `===== 3 passed, 1 failed, 1 skipped in 2.10s =====`
/// and the `FAILED <path>::<test>` lines.
pub(super) fn parse_pytest(stdout: &str, stderr: &str) -> TestSummary {
    let combined = combine(stdout, stderr);
    let mut s = TestSummary::default();

    let mut found_summary = false;
    for raw in combined.lines() {
        let line = raw.trim();
        // Stripped summary line: drop `=` markers then split.
        let stripped = line.trim_matches('=').trim();
        if stripped.contains(" in ")
            && (stripped.contains(" passed")
                || stripped.contains(" failed")
                || stripped.contains(" error")
                || stripped.contains(" skipped"))
        {
            let payload = stripped.split(" in ").next().unwrap_or(stripped);
            parse_label_count_pairs(payload, &mut s);
            found_summary = true;
        }
    }

    let mut failures: Vec<TestFailure> = Vec::new();
    for line in combined.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("FAILED ") {
            let mut parts = rest.split(" - ");
            let name_loc = parts.next().unwrap_or("").trim();
            let message = parts.next().unwrap_or("").trim().to_string();
            let (name, location) = if let Some((path, test)) = name_loc.split_once("::") {
                (test.trim().to_string(), Some(path.trim().to_string()))
            } else {
                (name_loc.to_string(), None)
            };
            failures.push(TestFailure {
                name,
                message,
                location,
            });
        }
    }
    if (s.failed as usize) < failures.len() {
        s.failed = failures.len().try_into().unwrap_or(u32::MAX);
    }
    s.failures = failures;

    if !found_summary && s.failures.is_empty() {
        s.parse_warning = Some("pytest summary line not found".to_string());
    }
    s
}

/// Parse `go test ./...` output. The standard form prints one
/// `--- FAIL: <Name> (Ns)` line per failure plus a per-package summary:
///   FAIL    pkg/path    0.123s
///   ok      pkg/other   0.001s
pub(super) fn parse_go(stdout: &str, stderr: &str) -> TestSummary {
    let combined = combine(stdout, stderr);
    let mut s = TestSummary::default();

    let mut failures: Vec<TestFailure> = Vec::new();
    for (i, line) in combined.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("--- FAIL: ") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            // The next non-blank line is usually the assertion / message.
            let message = combined
                .lines()
                .skip(i + 1)
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with("---") && !l.starts_with("FAIL"))
                .map(str::to_string)
                .unwrap_or_default();
            failures.push(TestFailure {
                name,
                message,
                location: None,
            });
        }
        if trimmed.starts_with("--- PASS:") {
            s.passed += 1;
        }
        if trimmed.starts_with("--- SKIP:") {
            s.skipped += 1;
        }
    }
    s.failed = failures.len().try_into().unwrap_or(u32::MAX);
    s.failures = failures;
    if s.passed == 0 && s.failed == 0 && s.skipped == 0 {
        s.parse_warning = Some("go test output not recognised; counts unavailable".to_string());
    }
    s
}

/// Generic fallback: scan for "N passed" / "N failed" / "N skipped"
/// substrings anywhere in the output. When nothing matches, counts stay
/// at zero and `parse_warning` is set.
pub(super) fn parse_generic(stdout: &str, stderr: &str) -> TestSummary {
    let combined = combine(stdout, stderr);
    let mut s = TestSummary::default();
    for line in combined.lines() {
        parse_label_count_pairs(line, &mut s);
    }
    if s.passed == 0 && s.failed == 0 && s.skipped == 0 {
        s.parse_warning = Some("unknown test runner; counts unavailable".to_string());
    }
    s
}

fn combine(stdout: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        return stdout.to_string();
    }
    if stdout.is_empty() {
        return stderr.to_string();
    }
    format!("{stdout}\n{stderr}")
}

/// Scan `payload` for `N <label>` segments (separated by commas,
/// semicolons, or whitespace) and add to `s.{passed, failed, skipped}`.
fn parse_label_count_pairs(payload: &str, s: &mut TestSummary) {
    let lower = payload.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split([',', ';']).collect();
    for tok in tokens {
        let tok = tok.trim();
        let Some((n, rest)) = split_first_number(tok) else {
            continue;
        };
        let rest = rest.trim_start();
        if rest.starts_with("passed") || rest.starts_with("passing") {
            s.passed += n;
        } else if rest.starts_with("failed")
            || rest.starts_with("failing")
            || rest.starts_with("error")
        {
            s.failed += n;
        } else if rest.starts_with("skipped")
            || rest.starts_with("pending")
            || rest.starts_with("ignored")
        {
            s.skipped += n;
        }
    }
}

/// Split a token like "3 passed" into `(3, "passed")`. Returns `None`
/// when the leading characters don't form a non-negative integer.
fn split_first_number(s: &str) -> Option<(u32, &str)> {
    let s = s.trim_start();
    let mut idx = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() {
            idx = i + 1;
        } else {
            break;
        }
    }
    if idx == 0 {
        return None;
    }
    let n: u32 = s[..idx].parse().ok()?;
    Some((n, s[idx..].trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_passing() {
        let out = "
running 3 tests
test foo::a ... ok
test foo::b ... ok
test foo::c ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";
        let s = parse_cargo(out, "");
        assert_eq!(s.passed, 3);
        assert_eq!(s.failed, 0);
        assert!(s.failures.is_empty());
    }

    #[test]
    fn cargo_with_failure() {
        let out = "
running 2 tests
test login ... FAILED
test logout ... ok

failures:

---- login stdout ----
assertion failed: response.is_ok()
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    login

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";
        let s = parse_cargo(out, "");
        assert_eq!(s.passed, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.failures.len(), 1);
        assert_eq!(s.failures[0].name, "login");
        assert!(s.failures[0].message.contains("assertion failed"));
    }

    #[test]
    fn pytest_summary_with_failure() {
        let out = "
==================== test session starts ====================
collected 4 items

tests/test_login.py::test_a PASSED
tests/test_login.py::test_b FAILED

================ FAILURES =================
FAILED tests/test_login.py::test_b - AssertionError: nope

==================== 1 failed, 3 passed in 0.05s ===========
";
        let s = parse_pytest(out, "");
        assert_eq!(s.passed, 3);
        assert_eq!(s.failed, 1);
        assert_eq!(s.failures.len(), 1);
        assert_eq!(s.failures[0].name, "test_b");
        assert_eq!(
            s.failures[0].location.as_deref(),
            Some("tests/test_login.py")
        );
    }

    #[test]
    fn go_test_with_failure() {
        let out = "
=== RUN   TestA
--- PASS: TestA (0.00s)
=== RUN   TestB
--- FAIL: TestB (0.01s)
    main_test.go:12: expected 1 got 0
FAIL
exit status 1
FAIL    example.com/pkg    0.012s
";
        let s = parse_go(out, "");
        assert_eq!(s.passed, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.failures.len(), 1);
        assert_eq!(s.failures[0].name, "TestB");
        assert!(s.failures[0].message.contains("expected"));
    }

    #[test]
    fn jest_style_summary() {
        let out = "
Tests:       2 failed, 3 passed, 1 skipped, 6 total
Snapshots:   0 total
Time:        0.5 s
";
        let s = parse_npm(out, "");
        assert_eq!(s.passed, 3);
        assert_eq!(s.failed, 2);
        assert_eq!(s.skipped, 1);
    }

    #[test]
    fn generic_falls_back_to_warning() {
        let s = parse_generic("nothing matches here", "");
        assert!(s.parse_warning.is_some());
        assert_eq!(s.passed + s.failed + s.skipped, 0);
    }

    #[test]
    fn split_first_number_basic() {
        assert_eq!(split_first_number("3 passed"), Some((3, "passed")));
        assert_eq!(split_first_number("0 failed"), Some((0, "failed")));
        assert_eq!(split_first_number("not a number"), None);
    }
}
