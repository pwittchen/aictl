//! Compute a unified diff between two text files.
//!
//! The model hands in two file paths, one per line, and the tool returns a
//! standard unified diff with 3 lines of context. Useful before applying an
//! `edit_file` change, or to summarize what differs between two revisions
//! of a file without shelling out to `diff`.
//!
//! The diff is computed in-process via an LCS (longest common subsequence)
//! DP table over the lines of each file — no external `diff` binary is
//! invoked, so the tool works the same on every platform. LCS is `O(N*M)`
//! in time and memory, so the implementation refuses to diff files longer
//! than [`MAX_LINES`] each to keep memory bounded.

use std::fmt::Write as _;

use super::util::truncate_output;

const CONTEXT_LINES: usize = 3;

/// Upper bound on lines per file. The LCS DP table is `(N+1) * (M+1)`
/// `usize` cells (8 bytes each on 64-bit), so at 2000 lines per side the
/// worst case is ~32 MB — safe to allocate on any host we target.
const MAX_LINES: usize = 2000;

pub(super) async fn tool_diff_files(input: &str) -> String {
    let input = input.trim();
    let Some((path_a, rest)) = input.split_once('\n') else {
        return "Invalid input: expected two file paths, one per line".to_string();
    };
    let path_a = path_a.trim();
    let path_b = match rest.split_once('\n') {
        Some((b, _)) => b.trim(),
        None => rest.trim(),
    };
    if path_a.is_empty() || path_b.is_empty() {
        return "Invalid input: expected two file paths, one per line".to_string();
    }

    let content_a = match tokio::fs::read_to_string(path_a).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading file '{path_a}': {e}"),
    };
    let content_b = match tokio::fs::read_to_string(path_b).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading file '{path_b}': {e}"),
    };

    let lines_a: Vec<&str> = content_a.lines().collect();
    let lines_b: Vec<&str> = content_b.lines().collect();

    if lines_a.len() > MAX_LINES || lines_b.len() > MAX_LINES {
        return format!(
            "Error: file too large to diff (max {MAX_LINES} lines per file; got {} and {})",
            lines_a.len(),
            lines_b.len()
        );
    }

    let mut diff = unified_diff(path_a, path_b, &lines_a, &lines_b);
    if diff.is_empty() {
        return "(files are identical)".to_string();
    }
    truncate_output(&mut diff);
    diff
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Equal(usize),
    Delete(usize),
    Insert(usize),
}

fn compute_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let n_a = a.len();
    let n_b = b.len();

    let mut dp = vec![vec![0usize; n_b + 1]; n_a + 1];
    for row in 0..n_a {
        for col in 0..n_b {
            dp[row + 1][col + 1] = if a[row] == b[col] {
                dp[row][col] + 1
            } else {
                dp[row + 1][col].max(dp[row][col + 1])
            };
        }
    }

    let mut ops = Vec::new();
    let mut row = n_a;
    let mut col = n_b;
    while row > 0 || col > 0 {
        if row > 0 && col > 0 && a[row - 1] == b[col - 1] {
            ops.push(Op::Equal(row - 1));
            row -= 1;
            col -= 1;
        } else if col > 0 && (row == 0 || dp[row][col - 1] >= dp[row - 1][col]) {
            ops.push(Op::Insert(col - 1));
            col -= 1;
        } else {
            ops.push(Op::Delete(row - 1));
            row -= 1;
        }
    }
    ops.reverse();
    ops
}

fn unified_diff(path_a: &str, path_b: &str, a: &[&str], b: &[&str]) -> String {
    let ops = compute_ops(a, b);
    let has_changes = ops.iter().any(|op| !matches!(op, Op::Equal(_)));
    if !has_changes {
        return String::new();
    }

    let mut line_info: Vec<(usize, usize)> = Vec::with_capacity(ops.len());
    let mut ai = 1usize;
    let mut bi = 1usize;
    for op in &ops {
        line_info.push((ai, bi));
        match op {
            Op::Equal(_) => {
                ai += 1;
                bi += 1;
            }
            Op::Delete(_) => ai += 1,
            Op::Insert(_) => bi += 1,
        }
    }

    let hunks = group_hunks(&ops, CONTEXT_LINES);

    let mut out = String::new();
    let _ = writeln!(out, "--- {path_a}");
    let _ = writeln!(out, "+++ {path_b}");
    for (start, end) in hunks {
        let (a_start, b_start) = line_info[start];
        let mut a_count = 0usize;
        let mut b_count = 0usize;
        for op in &ops[start..=end] {
            match op {
                Op::Equal(_) => {
                    a_count += 1;
                    b_count += 1;
                }
                Op::Delete(_) => a_count += 1,
                Op::Insert(_) => b_count += 1,
            }
        }
        let a_display = if a_count == 0 {
            a_start.saturating_sub(1)
        } else {
            a_start
        };
        let b_display = if b_count == 0 {
            b_start.saturating_sub(1)
        } else {
            b_start
        };
        let _ = writeln!(out, "@@ -{a_display},{a_count} +{b_display},{b_count} @@");
        for op in &ops[start..=end] {
            match op {
                Op::Equal(ai) => {
                    out.push(' ');
                    out.push_str(a[*ai]);
                    out.push('\n');
                }
                Op::Delete(ai) => {
                    out.push('-');
                    out.push_str(a[*ai]);
                    out.push('\n');
                }
                Op::Insert(bi) => {
                    out.push('+');
                    out.push_str(b[*bi]);
                    out.push('\n');
                }
            }
        }
    }
    out
}

fn group_hunks(ops: &[Op], context: usize) -> Vec<(usize, usize)> {
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(i, op)| (!matches!(op, Op::Equal(_))).then_some(i))
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }

    let last = ops.len() - 1;
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    let mut cur_start = changed[0].saturating_sub(context);
    let mut cur_end = (changed[0] + context).min(last);

    for &idx in &changed[1..] {
        let start = idx.saturating_sub(context);
        let end = (idx + context).min(last);
        if start <= cur_end.saturating_add(1) {
            cur_end = end.max(cur_end);
        } else {
            hunks.push((cur_start, cur_end));
            cur_start = start;
            cur_end = end;
        }
    }
    hunks.push((cur_start, cur_end));
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    #[test]
    fn identical_files_produce_empty_diff() {
        let a = split("one\ntwo\nthree\n");
        let b = split("one\ntwo\nthree\n");
        assert_eq!(unified_diff("a", "b", &a, &b), "");
    }

    #[test]
    fn single_line_change() {
        let a = split("one\ntwo\nthree\n");
        let b = split("one\nTWO\nthree\n");
        let out = unified_diff("a.txt", "b.txt", &a, &b);
        assert!(out.starts_with("--- a.txt\n+++ b.txt\n@@ "));
        assert!(out.contains("-two"));
        assert!(out.contains("+TWO"));
        assert!(out.contains(" one"));
        assert!(out.contains(" three"));
    }

    #[test]
    fn pure_insertion_at_end() {
        let a = split("one\ntwo\n");
        let b = split("one\ntwo\nthree\n");
        let out = unified_diff("a", "b", &a, &b);
        assert!(out.contains("+three"));
        // No `-` line removals — only the `--- a` header should match.
        let removals = out
            .lines()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count();
        assert_eq!(removals, 0);
    }

    #[test]
    fn pure_deletion() {
        let a = split("one\ntwo\nthree\n");
        let b = split("one\nthree\n");
        let out = unified_diff("a", "b", &a, &b);
        assert!(out.contains("-two"));
    }

    #[test]
    fn hunks_merged_when_close() {
        let a = split("a\nb\nc\nd\ne\nf\ng\n");
        let b = split("a\nX\nc\nd\ne\nY\ng\n");
        let out = unified_diff("a", "b", &a, &b);
        // Only 3 unchanged lines between changes — within 2*context, merge into one hunk
        assert_eq!(out.matches("@@").count(), 2); // one header = two "@@" markers
    }

    #[test]
    fn hunks_separate_when_far_apart() {
        let a = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut b_vec: Vec<String> = a.lines().map(String::from).collect();
        b_vec[1] = "CHANGED1".into();
        b_vec[18] = "CHANGED2".into();
        let b = b_vec.join("\n");
        let a_lines: Vec<&str> = a.lines().collect();
        let b_lines: Vec<&str> = b.lines().collect();
        let out = unified_diff("a", "b", &a_lines, &b_lines);
        // Two distinct hunks → two "@@ ... @@" lines (four "@@")
        assert_eq!(out.matches("@@").count(), 4);
    }

    #[test]
    fn empty_to_content_is_pure_insertion() {
        let a: Vec<&str> = Vec::new();
        let b = split("one\ntwo\n");
        let out = unified_diff("a", "b", &a, &b);
        assert!(out.contains("@@ -0,0 +1,2 @@"));
        assert!(out.contains("+one"));
        assert!(out.contains("+two"));
    }

    #[test]
    fn content_to_empty_is_pure_deletion() {
        let a = split("one\ntwo\n");
        let b: Vec<&str> = Vec::new();
        let out = unified_diff("a", "b", &a, &b);
        assert!(out.contains("@@ -1,2 +0,0 @@"));
        assert!(out.contains("-one"));
        assert!(out.contains("-two"));
    }

    #[tokio::test]
    async fn tool_reports_identical() {
        let dir = std::env::temp_dir().join(format!("aictl_diff_eq_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "x\ny\n").unwrap();
        std::fs::write(&b, "x\ny\n").unwrap();
        let input = format!("{}\n{}", a.display(), b.display());
        let out = tool_diff_files(&input).await;
        assert_eq!(out, "(files are identical)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tool_reports_diff() {
        let dir = std::env::temp_dir().join(format!("aictl_diff_ne_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "hello\nworld\n").unwrap();
        std::fs::write(&b, "hello\nRust\n").unwrap();
        let input = format!("{}\n{}", a.display(), b.display());
        let out = tool_diff_files(&input).await;
        assert!(out.contains("--- "));
        assert!(out.contains("+++ "));
        assert!(out.contains("-world"));
        assert!(out.contains("+Rust"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tool_missing_second_path() {
        let out = tool_diff_files("only-one-path").await;
        assert!(out.contains("Invalid input"));
    }

    #[tokio::test]
    async fn tool_missing_file() {
        let input = "/tmp/aictl_nonexistent_a_xyz\n/tmp/aictl_nonexistent_b_xyz";
        let out = tool_diff_files(input).await;
        assert!(out.starts_with("Error reading file"));
    }
}
