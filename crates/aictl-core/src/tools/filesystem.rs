use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::config::MAX_TOOL_OUTPUT_LEN;

use super::util::truncate_output;

// ============================================================================
// rg availability probe (shared by search_files and find_files)
// ============================================================================

fn rg_available() -> bool {
    static RG_AVAILABLE: OnceLock<bool> = OnceLock::new();
    *RG_AVAILABLE.get_or_init(|| {
        if std::env::var("AICTL_TEST_FORCE_RG_FALLBACK").is_ok() {
            return false;
        }
        std::process::Command::new("rg")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

// ============================================================================
// read_file
// ============================================================================

#[derive(Debug, PartialEq, Eq)]
enum LinesSpec {
    All,
    Range(usize, Option<usize>),
}

fn parse_read_input(input: &str) -> (&str, Option<LinesSpec>) {
    let mut iter = input.splitn(2, '\n');
    let path = iter.next().unwrap_or("").trim();
    match iter.next() {
        None => (path, None),
        Some(rest) => {
            let rest = rest.trim();
            if rest == "--lines" {
                return (path, Some(LinesSpec::All));
            }
            if let Some(arg) = rest.strip_prefix("--lines") {
                let arg = arg.trim();
                if arg.is_empty() {
                    return (path, Some(LinesSpec::All));
                }
                if let Some((s, e)) = arg.split_once('-') {
                    let start = s.trim().parse::<usize>().unwrap_or(0);
                    let end = e.trim().parse::<usize>().unwrap_or(0);
                    return (path, Some(LinesSpec::Range(start, Some(end))));
                }
                let n = arg.parse::<usize>().unwrap_or(0);
                return (path, Some(LinesSpec::Range(n, None)));
            }
            (path, None)
        }
    }
}

fn render_numbered(contents: &str, range: Option<(usize, Option<usize>)>) -> String {
    let mut lines: Vec<&str> = if contents.is_empty() {
        Vec::new()
    } else {
        contents.split('\n').collect()
    };
    if !contents.is_empty() && contents.ends_with('\n') && lines.last() == Some(&"") {
        lines.pop();
    }
    let total = lines.len();

    // None → full file; Some((s, None)) → single line s; Some((s, Some(e))) → range s..=e
    let (start, req_end) = match range {
        None => (1usize, total.max(1)),
        Some((s, None)) => (s, s),
        Some((s, Some(e))) => (s, e),
    };

    if start == 0 {
        return "Error: lines are 1-based — use --lines 1 instead of --lines 0".to_string();
    }
    if total == 0 {
        return "(empty file)".to_string();
    }
    if start > total {
        return format!("(file ends at line {total}, no content)");
    }
    if req_end < start {
        return format!("Error: invalid range — end ({req_end}) must be ≥ start ({start})");
    }
    let actual_end = req_end.min(total);
    let clamped = req_end > total;

    let mut out = String::with_capacity(contents.len() + (actual_end - start + 1) * 8);
    for i in start..=actual_end {
        let _ = writeln!(out, "{:>5}: {}", i, lines[i - 1]);
    }
    if clamped {
        let _ = write!(out, "(end of file at line {total})");
    }
    out
}

pub(super) async fn tool_read_file(input: &str) -> String {
    let input = input.trim();
    let (path, lines_spec) = parse_read_input(input);

    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading file: {e}"),
    };

    let mut output = match lines_spec {
        None => {
            if contents.is_empty() {
                "(empty file)".to_string()
            } else {
                contents
            }
        }
        Some(LinesSpec::All) => render_numbered(&contents, None),
        Some(LinesSpec::Range(s, e)) => render_numbered(&contents, Some((s, e))),
    };

    truncate_output(&mut output);
    output
}

// ============================================================================
// write_file / remove_file / create_directory / list_directory
// ============================================================================

pub(super) async fn tool_write_file(input: &str) -> String {
    let input = input.trim();
    match input.split_once('\n') {
        Some((path, content)) => {
            let path = path.trim();
            match tokio::fs::write(path, content).await {
                Ok(()) => format!("Wrote {} bytes to {path}", content.len()),
                Err(e) => format!("Error writing file: {e}"),
            }
        }
        None => "Invalid input: expected first line as file path, remaining lines as content"
            .to_string(),
    }
}

pub(super) async fn tool_remove_file(input: &str) -> String {
    let path = input.trim();
    match tokio::fs::remove_file(path).await {
        Ok(()) => format!("Removed {path}"),
        Err(e) => format!("Error removing file: {e}"),
    }
}

pub(super) async fn tool_create_directory(input: &str) -> String {
    let path = input.trim();
    match tokio::fs::create_dir_all(path).await {
        Ok(()) => format!("Created directory {path}"),
        Err(e) => format!("Error creating directory: {e}"),
    }
}

pub(super) async fn tool_list_directory(input: &str) -> String {
    let path = input.trim();
    let path = if path.is_empty() { "." } else { path };
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => {
            let mut result = String::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let prefix = match entry.file_type().await {
                    Ok(ft) if ft.is_dir() => "[DIR]",
                    Ok(ft) if ft.is_symlink() => "[LINK]",
                    _ => "[FILE]",
                };
                let _ = writeln!(result, "{prefix}  {name}");
            }
            if result.is_empty() {
                "(empty directory)".to_string()
            } else {
                result
            }
        }
        Err(e) => format!("Error listing directory: {e}"),
    }
}

// ============================================================================
// search_files
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaseMode {
    Sensitive,
    Smart,
    Insensitive,
}

#[derive(Debug)]
struct SearchArgs {
    pattern: String,
    dir: String,
    regex: bool,
    case: CaseMode,
    file_type: Option<String>,
    max: usize,
    context: usize,
    no_ignore: bool,
}

fn parse_search_args(input: &str) -> SearchArgs {
    let mut args = SearchArgs {
        pattern: String::new(),
        dir: ".".to_string(),
        regex: false,
        case: CaseMode::Smart,
        file_type: None,
        max: 200,
        context: 0,
        no_ignore: false,
    };
    let mut iter = input.lines();
    args.pattern = iter.next().unwrap_or("").to_string();
    let mut dir_set = false;
    for line in iter {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("--") {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let flag = parts.next().unwrap_or("");
            let val = parts.next().map_or("", str::trim);
            match flag {
                "regex" => args.regex = true,
                "literal" => args.regex = false,
                "case" => {
                    args.case = match val {
                        "sensitive" => CaseMode::Sensitive,
                        "insensitive" => CaseMode::Insensitive,
                        _ => CaseMode::Smart,
                    };
                }
                "type" => {
                    if !val.is_empty() {
                        args.file_type = Some(val.to_string());
                    }
                }
                "max" => {
                    if let Ok(n) = val.parse::<usize>() {
                        args.max = n.min(1000);
                    }
                }
                "context" => {
                    if let Ok(n) = val.parse::<usize>() {
                        args.context = n;
                    }
                }
                "no-ignore" => args.no_ignore = true,
                _ => {}
            }
        } else if !dir_set {
            args.dir = line.to_string();
            dir_set = true;
        }
    }
    if args.dir.is_empty() {
        args.dir = ".".to_string();
    }
    args
}

pub(super) async fn tool_search_files(input: &str) -> String {
    let input = input.trim();
    let args = parse_search_args(input);
    tokio::task::spawn_blocking(move || {
        if rg_available() {
            run_rg_search(&args)
        } else {
            search_files_fallback(&args)
        }
    })
    .await
    .unwrap_or_else(|e| format!("Error running search: {e}"))
}

fn run_rg_search(args: &SearchArgs) -> String {
    let mut cmd = std::process::Command::new("rg");
    cmd.arg("--no-heading")
        .arg("--line-number")
        .arg("--color=never")
        .arg("--hidden");
    if !args.regex {
        cmd.arg("--fixed-strings");
    }
    match args.case {
        CaseMode::Smart => {
            cmd.arg("--smart-case");
        }
        CaseMode::Sensitive => {
            cmd.arg("--case-sensitive");
        }
        CaseMode::Insensitive => {
            cmd.arg("--ignore-case");
        }
    }
    if let Some(t) = &args.file_type {
        cmd.arg("--type").arg(t);
    }
    if args.context > 0 {
        cmd.arg("-C").arg(args.context.to_string());
    }
    if args.no_ignore {
        cmd.arg("--no-ignore");
    }
    cmd.arg("--max-count").arg(args.max.to_string());
    cmd.arg("--").arg(&args.pattern).arg(&args.dir);

    match cmd.output() {
        Ok(out) => {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                if stdout.trim().is_empty() {
                    "No matches found.".to_string()
                } else {
                    let mut s = stdout;
                    truncate_output(&mut s);
                    s
                }
            } else if out.status.code() == Some(1) {
                "No matches found.".to_string()
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let err = stderr.trim();
                if err.is_empty() {
                    format!("Error: rg exited with status {:?}", out.status.code())
                } else {
                    format!("Error: {err}")
                }
            }
        }
        Err(e) => format!("Error running rg: {e}"),
    }
}

fn search_files_fallback(args: &SearchArgs) -> String {
    let glob_pattern = format!("{}/**/*", args.dir.trim_end_matches('/'));
    let entries = match glob::glob(&glob_pattern) {
        Ok(paths) => paths,
        Err(e) => return format!("Error: invalid path pattern: {e}"),
    };

    let regex = if args.regex {
        match regex::Regex::new(&args.pattern) {
            Ok(r) => Some(r),
            Err(e) => return format!("Error: invalid regex: {e}"),
        }
    } else {
        None
    };

    let pattern_lower = args.pattern.to_lowercase();
    let case_insensitive = matches!(args.case, CaseMode::Insensitive)
        || (matches!(args.case, CaseMode::Smart) && args.pattern == pattern_lower);

    let mut result = String::new();
    let mut count = 0usize;

    'outer: for entry in entries {
        let Ok(path) = entry else { continue };
        if !path.is_file() {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let path_str = path.to_string_lossy();
        for (i, line) in contents.lines().enumerate() {
            let hit = if let Some(re) = &regex {
                re.is_match(line)
            } else if case_insensitive {
                line.to_lowercase().contains(&pattern_lower)
            } else {
                line.contains(&args.pattern)
            };
            if !hit {
                continue;
            }
            if !result.is_empty() {
                result.push('\n');
            }
            let _ = write!(result, "{path_str}:{}:{line}", i + 1);
            count += 1;
            if count >= args.max {
                result.push_str("\n... (max matches reached)");
                break 'outer;
            }
            if result.len() > MAX_TOOL_OUTPUT_LEN {
                break 'outer;
            }
        }
    }

    if result.is_empty() {
        "No matches found.".to_string()
    } else {
        truncate_output(&mut result);
        result
    }
}

// ============================================================================
// find_files
// ============================================================================

#[derive(Debug)]
struct FindArgs {
    pattern: String,
    base_dir: String,
    file_type: Option<String>,
}

fn parse_find_args(input: &str) -> FindArgs {
    let mut args = FindArgs {
        pattern: String::new(),
        base_dir: ".".to_string(),
        file_type: None,
    };
    let mut iter = input.lines();
    args.pattern = iter.next().unwrap_or("").trim().to_string();
    let mut dir_set = false;
    for line in iter {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("--") {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let flag = parts.next().unwrap_or("");
            let val = parts.next().map_or("", str::trim);
            if flag == "type" && !val.is_empty() {
                args.file_type = Some(val.to_string());
            }
        } else if !dir_set {
            args.base_dir = line.to_string();
            dir_set = true;
        }
    }
    if args.base_dir.is_empty() {
        args.base_dir = ".".to_string();
    }
    args
}

pub(super) fn tool_find_files(input: &str) -> String {
    let input = input.trim();
    let args = parse_find_args(input);
    if rg_available() {
        run_rg_files(&args)
    } else {
        find_files_glob_fallback(&args)
    }
}

fn run_rg_files(args: &FindArgs) -> String {
    let mut cmd = std::process::Command::new("rg");
    cmd.arg("--files").arg("--hidden");
    if let Some(t) = &args.file_type {
        cmd.arg("--type").arg(t);
    }
    cmd.arg("--").arg(&args.base_dir);

    match cmd.output() {
        Ok(out) => {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let err = stderr.trim();
                if !err.is_empty() && err.contains("unrecognized file type") {
                    return find_files_glob_fallback(args);
                }
                if err.is_empty() {
                    return format!("Error: rg exited with status {:?}", out.status.code());
                }
                return format!("Error: {err}");
            }
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

            // When --type drives the filter, rg has already filtered for us.
            if args.file_type.is_some() {
                return if stdout.trim().is_empty() {
                    "No matches found.".to_string()
                } else {
                    let mut s = stdout;
                    truncate_output(&mut s);
                    s
                };
            }

            // No pattern → return rg's full file list.
            if args.pattern.is_empty() {
                return if stdout.trim().is_empty() {
                    "No matches found.".to_string()
                } else {
                    let mut s = stdout;
                    truncate_output(&mut s);
                    s
                };
            }

            let direct = glob::Pattern::new(&args.pattern).ok();
            let combined_str = format!("{}/{}", args.base_dir.trim_end_matches('/'), args.pattern);
            let combined = glob::Pattern::new(&combined_str).ok();
            if direct.is_none() && combined.is_none() {
                return format!("Error: invalid glob pattern: {}", args.pattern);
            }

            let dir_prefix_slash = if args.base_dir.ends_with('/') {
                args.base_dir.clone()
            } else {
                format!("{}/", args.base_dir)
            };

            let mut result = String::new();
            for path in stdout.lines() {
                let path = path.trim();
                if path.is_empty() {
                    continue;
                }
                let rel = path
                    .strip_prefix(&dir_prefix_slash)
                    .or_else(|| path.strip_prefix("./"))
                    .unwrap_or(path);
                let matched = direct.as_ref().is_some_and(|p| p.matches(rel))
                    || direct.as_ref().is_some_and(|p| p.matches(path))
                    || combined.as_ref().is_some_and(|p| p.matches(path));
                if matched {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(path);
                    if result.len() > MAX_TOOL_OUTPUT_LEN {
                        break;
                    }
                }
            }

            if result.is_empty() {
                "No matches found.".to_string()
            } else {
                truncate_output(&mut result);
                result
            }
        }
        Err(e) => format!("Error running rg: {e}"),
    }
}

fn find_files_glob_fallback(args: &FindArgs) -> String {
    let full_pattern = if std::path::Path::new(&args.pattern).is_absolute() {
        args.pattern.clone()
    } else {
        format!("{}/{}", args.base_dir.trim_end_matches('/'), args.pattern)
    };
    match glob::glob(&full_pattern) {
        Ok(paths) => {
            let mut result = String::new();
            for entry in paths {
                match entry {
                    Ok(path) => {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        result.push_str(&path.to_string_lossy());
                    }
                    Err(e) => {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        let _ = write!(result, "(error: {e})");
                    }
                }
                if result.len() > MAX_TOOL_OUTPUT_LEN {
                    truncate_output(&mut result);
                    return result;
                }
            }
            if result.is_empty() {
                "No matches found.".to_string()
            } else {
                result
            }
        }
        Err(e) => format!("Error parsing glob pattern: {e}"),
    }
}

// ============================================================================
// edit_file
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditBlock {
    line_range: Option<(usize, usize)>,
    old: String,
    new: String,
}

fn parse_line_range(s: &str) -> Result<(usize, usize), String> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once('-') {
        let a = a.trim();
        let b = b.trim();
        if a.is_empty() || b.is_empty() {
            return Err(format!("Invalid line range: '@{s}' (expected @N or @N-M)"));
        }
        let start: usize = a
            .parse()
            .map_err(|_| format!("Invalid line range: '@{s}'"))?;
        let end: usize = b
            .parse()
            .map_err(|_| format!("Invalid line range: '@{s}'"))?;
        if start == 0 {
            return Err(format!("Invalid line range: '@{s}' (lines are 1-based)"));
        }
        if end < start {
            return Err(format!("Invalid line range: '@{s}' (end must be ≥ start)"));
        }
        Ok((start, end))
    } else {
        let n: usize = s
            .parse()
            .map_err(|_| format!("Invalid line range: '@{s}'"))?;
        if n == 0 {
            return Err(format!("Invalid line range: '@{s}' (lines are 1-based)"));
        }
        Ok((n, n))
    }
}

fn parse_edit_body(body: &str) -> Result<Vec<EditBlock>, String> {
    let mut blocks = Vec::new();
    let mut rest = body;

    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }

        let mut line_range = None;
        if let Some(after_at) = rest.strip_prefix('@') {
            let nl = after_at.find('\n').unwrap_or(after_at.len());
            let rng_str = &after_at[..nl];
            line_range = Some(parse_line_range(rng_str)?);
            rest = &after_at[nl..];
            rest = rest.trim_start();
        }

        let after_open = rest
            .strip_prefix("<<<")
            .ok_or_else(|| "Invalid input: expected <<< delimiter after file path".to_string())?;

        let close_idx = after_open
            .find(">>>")
            .ok_or_else(|| "Invalid input: expected >>> closing delimiter".to_string())?;
        let body_part = &after_open[..close_idx];
        let next_rest = &after_open[close_idx + 3..];

        let (old_text, new_text) = body_part.split_once("===").ok_or_else(|| {
            "Invalid input: expected === separator between old and new text".to_string()
        })?;
        let old_text = old_text.strip_prefix('\n').unwrap_or(old_text);
        let old_text = old_text.strip_suffix('\n').unwrap_or(old_text);
        let new_text = new_text.strip_prefix('\n').unwrap_or(new_text);
        let new_text = new_text.strip_suffix('\n').unwrap_or(new_text);

        blocks.push(EditBlock {
            line_range,
            old: old_text.to_string(),
            new: new_text.to_string(),
        });
        rest = next_rest;
    }

    if blocks.is_empty() {
        return Err("Invalid input: expected at least one <<< ... === ... >>> block".to_string());
    }
    Ok(blocks)
}

/// Map a 1-based, inclusive line range to byte offsets inside `contents`.
/// Returns `None` when `start` is past EOF.
fn line_range_bytes(contents: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    if start == 0 {
        return None;
    }
    let bytes = contents.as_bytes();
    let mut current_line: usize = 1;
    let mut start_byte: Option<usize> = if start == 1 { Some(0) } else { None };
    let mut end_byte: usize = bytes.len();

    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            current_line += 1;
            if start_byte.is_none() && current_line == start {
                start_byte = Some(i + 1);
            }
            if current_line == end + 1 {
                end_byte = i;
                return Some((start_byte?, end_byte));
            }
        }
    }
    let sb = start_byte?;
    if sb > end_byte {
        end_byte = sb;
    }
    Some((sb, end_byte))
}

enum FuzzyResult {
    None,
    Unique { start: usize, end: usize },
    Multiple(usize),
}

fn normalize_line_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Line-based fuzzy locator: collapses whitespace per line and slides a
/// window over `scope`. Used only when an exact match returned zero hits.
fn fuzzy_locate(scope: &str, old: &str) -> FuzzyResult {
    if old.is_empty() {
        return FuzzyResult::None;
    }

    let mut lines_with_offsets: Vec<(usize, &str)> = Vec::new();
    let mut off = 0usize;
    for line in scope.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        lines_with_offsets.push((off, trimmed));
        off += line.len();
    }
    if lines_with_offsets.is_empty() {
        return FuzzyResult::None;
    }

    let mut old_lines: Vec<&str> = old.split('\n').collect();
    if old_lines.last() == Some(&"") && old_lines.len() > 1 {
        old_lines.pop();
    }

    let old_norm: Vec<String> = old_lines.iter().map(|l| normalize_line_ws(l)).collect();
    let file_norm: Vec<String> = lines_with_offsets
        .iter()
        .map(|(_, l)| normalize_line_ws(l))
        .collect();

    let n = old_norm.len();
    let m = file_norm.len();
    if n == 0 || n > m {
        return FuzzyResult::None;
    }

    let mut matches = Vec::new();
    for i in 0..=m - n {
        if file_norm[i..i + n] == old_norm[..] {
            matches.push(i);
        }
    }

    match matches.len() {
        0 => FuzzyResult::None,
        1 => {
            let start_idx = matches[0];
            let end_idx = start_idx + n;
            let start = lines_with_offsets[start_idx].0;
            let end = if end_idx < lines_with_offsets.len() {
                lines_with_offsets[end_idx].0
            } else {
                scope.len()
            };
            let end = if end > start && scope.as_bytes().get(end - 1) == Some(&b'\n') {
                end - 1
            } else {
                end
            };
            FuzzyResult::Unique { start, end }
        }
        k => FuzzyResult::Multiple(k),
    }
}

fn apply_single_block(
    contents: &str,
    block: &EditBlock,
    idx: usize,
    is_single: bool,
) -> Result<String, String> {
    let prefix = if is_single {
        String::new()
    } else {
        format!("block {idx}: ")
    };

    let (search_start, search_end) = if let Some((s, e)) = block.line_range {
        if let Some(r) = line_range_bytes(contents, s, e) {
            r
        } else {
            let total = contents.lines().count();
            return Err(format!(
                "Error: {prefix}line range out of bounds: @{s}-{e} (file has {total} lines)"
            ));
        }
    } else {
        (0, contents.len())
    };

    let scope = &contents[search_start..search_end];
    let count = scope.matches(&block.old).count();

    if count == 1 {
        let pos_in_scope = scope.find(&block.old).unwrap();
        let pos = search_start + pos_in_scope;
        let mut updated = String::with_capacity(contents.len() - block.old.len() + block.new.len());
        updated.push_str(&contents[..pos]);
        updated.push_str(&block.new);
        updated.push_str(&contents[pos + block.old.len()..]);
        return Ok(updated);
    }
    if count > 1 {
        return Err(format!(
            "Error: {prefix}old text found {count} times in file — provide more context to match uniquely"
        ));
    }

    // count == 0: try fuzzy fallback (whitespace-normalized line match).
    match fuzzy_locate(scope, &block.old) {
        FuzzyResult::Unique { start, end } => {
            let pos = search_start + start;
            let end_pos = search_start + end;
            let mut updated =
                String::with_capacity(contents.len() - (end_pos - pos) + block.new.len());
            updated.push_str(&contents[..pos]);
            updated.push_str(&block.new);
            updated.push_str(&contents[end_pos..]);
            Ok(updated)
        }
        FuzzyResult::Multiple(n) => Err(format!(
            "Error: {prefix}old text not found exactly; fuzzy match found {n} candidates — narrow with more context or use @start-end"
        )),
        FuzzyResult::None => {
            if is_single {
                Err("Error: old text not found in file".to_string())
            } else {
                Err(format!("Error: {prefix}old text not found in file"))
            }
        }
    }
}

fn apply_edit_blocks(contents: &str, blocks: &[EditBlock]) -> Result<(String, usize), String> {
    let is_single = blocks.len() == 1;
    let mut current = contents.to_string();
    for (i, block) in blocks.iter().enumerate() {
        current = apply_single_block(&current, block, i + 1, is_single)?;
    }
    Ok((current, blocks.len()))
}

pub(super) async fn tool_edit_file(input: &str) -> String {
    let input = input.trim();
    let Some((path, rest)) = input.split_once('\n') else {
        return "Invalid input: expected file path on first line".to_string();
    };
    let path = path.trim();

    let blocks = match parse_edit_body(rest) {
        Ok(b) => b,
        Err(e) => return e,
    };

    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading file: {e}"),
    };

    let (updated, applied) = match apply_edit_blocks(&contents, &blocks) {
        Ok(r) => r,
        Err(e) => return e,
    };

    match tokio::fs::write(path, &updated).await {
        Ok(()) => {
            if applied == 1 {
                format!("Edited {path} (replaced 1 occurrence)")
            } else {
                format!("Edited {path} (applied {applied} blocks)")
            }
        }
        Err(e) => format!("Error writing file: {e}"),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- read_file parsing & rendering ---

    #[test]
    fn read_input_no_flag() {
        let (path, spec) = parse_read_input("src/lib.rs");
        assert_eq!(path, "src/lib.rs");
        assert!(spec.is_none());
    }

    #[test]
    fn read_input_lines_bare() {
        let (path, spec) = parse_read_input("src/lib.rs\n--lines");
        assert_eq!(path, "src/lib.rs");
        assert_eq!(spec, Some(LinesSpec::All));
    }

    #[test]
    fn read_input_lines_range() {
        let (path, spec) = parse_read_input("src/lib.rs\n--lines 10-20");
        assert_eq!(path, "src/lib.rs");
        assert_eq!(spec, Some(LinesSpec::Range(10, Some(20))));
    }

    #[test]
    fn read_input_lines_single() {
        let (_, spec) = parse_read_input("src/lib.rs\n--lines 42");
        assert_eq!(spec, Some(LinesSpec::Range(42, None)));
    }

    #[test]
    fn render_numbered_full_file() {
        let s = "alpha\nbeta\ngamma";
        let out = render_numbered(s, None);
        assert!(out.contains("    1: alpha"));
        assert!(out.contains("    2: beta"));
        assert!(out.contains("    3: gamma"));
    }

    #[test]
    fn render_numbered_range() {
        let s = "a\nb\nc\nd\ne";
        let out = render_numbered(s, Some((2, Some(4))));
        assert!(out.contains("    2: b"));
        assert!(out.contains("    3: c"));
        assert!(out.contains("    4: d"));
        assert!(!out.contains("    1: a"));
        assert!(!out.contains("    5: e"));
    }

    #[test]
    fn render_numbered_single_line() {
        let s = "a\nb\nc";
        let out = render_numbered(s, Some((2, None)));
        assert!(out.contains("    2: b"));
        assert!(!out.contains("a"));
        assert!(!out.contains("c"));
    }

    #[test]
    fn render_numbered_zero_errors() {
        let s = "a\nb\nc";
        let out = render_numbered(s, Some((0, None)));
        assert!(out.contains("Error: lines are 1-based"));
    }

    #[test]
    fn render_numbered_start_past_eof() {
        let s = "a\nb\nc";
        let out = render_numbered(s, Some((10, Some(20))));
        assert!(out.contains("(file ends at line 3, no content)"));
    }

    #[test]
    fn render_numbered_end_clamped() {
        let s = "a\nb\nc\nd\ne";
        let out = render_numbered(s, Some((4, Some(99))));
        assert!(out.contains("    4: d"));
        assert!(out.contains("    5: e"));
        assert!(out.contains("(end of file at line 5)"));
    }

    // --- search_files argument parsing ---

    #[test]
    fn parse_search_default() {
        let a = parse_search_args("needle");
        assert_eq!(a.pattern, "needle");
        assert_eq!(a.dir, ".");
        assert!(!a.regex);
        assert_eq!(a.case, CaseMode::Smart);
        assert_eq!(a.max, 200);
    }

    #[test]
    fn parse_search_pattern_and_dir() {
        let a = parse_search_args("needle\nsrc");
        assert_eq!(a.pattern, "needle");
        assert_eq!(a.dir, "src");
    }

    #[test]
    fn parse_search_flags() {
        let a = parse_search_args(
            "fn \\w+\n--regex\n--case sensitive\n--type rust\n--max 50\n--context 2\nsrc",
        );
        assert_eq!(a.pattern, "fn \\w+");
        assert!(a.regex);
        assert_eq!(a.case, CaseMode::Sensitive);
        assert_eq!(a.file_type.as_deref(), Some("rust"));
        assert_eq!(a.max, 50);
        assert_eq!(a.context, 2);
        assert_eq!(a.dir, "src");
    }

    #[test]
    fn parse_search_max_capped() {
        let a = parse_search_args("x\n--max 5000");
        assert_eq!(a.max, 1000);
    }

    // --- find_files argument parsing ---

    #[test]
    fn parse_find_default() {
        let a = parse_find_args("*.rs");
        assert_eq!(a.pattern, "*.rs");
        assert_eq!(a.base_dir, ".");
        assert!(a.file_type.is_none());
    }

    #[test]
    fn parse_find_with_type() {
        let a = parse_find_args("*.rs\n--type rust\nsrc");
        assert_eq!(a.pattern, "*.rs");
        assert_eq!(a.file_type.as_deref(), Some("rust"));
        assert_eq!(a.base_dir, "src");
    }

    // --- edit_file body parsing ---

    #[test]
    fn parse_edit_single_block() {
        let body = "<<<\nfoo\n===\nbar\n>>>";
        let blocks = parse_edit_body(body).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].old, "foo");
        assert_eq!(blocks[0].new, "bar");
        assert!(blocks[0].line_range.is_none());
    }

    #[test]
    fn parse_edit_two_blocks() {
        let body = "<<<\nfoo\n===\nbar\n>>>\n<<<\nbaz\n===\nqux\n>>>";
        let blocks = parse_edit_body(body).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].old, "foo");
        assert_eq!(blocks[1].old, "baz");
    }

    #[test]
    fn parse_edit_block_with_line_range() {
        let body = "@10-20\n<<<\nfoo\n===\nbar\n>>>";
        let blocks = parse_edit_body(body).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].line_range, Some((10, 20)));
    }

    #[test]
    fn parse_edit_block_with_single_line() {
        let body = "@42\n<<<\nfoo\n===\nbar\n>>>";
        let blocks = parse_edit_body(body).unwrap();
        assert_eq!(blocks[0].line_range, Some((42, 42)));
    }

    #[test]
    fn parse_edit_malformed_line_range_missing_end() {
        let body = "@42-\n<<<\nfoo\n===\nbar\n>>>";
        assert!(parse_edit_body(body).is_err());
    }

    #[test]
    fn parse_edit_zero_line_range_rejected() {
        let body = "@0\n<<<\nfoo\n===\nbar\n>>>";
        assert!(parse_edit_body(body).is_err());
    }

    #[test]
    fn parse_edit_range_without_block() {
        let body = "@10-20\n";
        assert!(parse_edit_body(body).is_err());
    }

    #[test]
    fn parse_edit_no_blocks() {
        assert!(parse_edit_body("").is_err());
    }

    // --- edit_file application ---

    #[test]
    fn apply_single_block_replaces_unique() {
        let contents = "hello world";
        let blocks = vec![EditBlock {
            line_range: None,
            old: "hello".into(),
            new: "goodbye".into(),
        }];
        let (out, n) = apply_edit_blocks(contents, &blocks).unwrap();
        assert_eq!(out, "goodbye world");
        assert_eq!(n, 1);
    }

    #[test]
    fn apply_two_blocks_in_order() {
        let contents = "aaa\nbbb\nccc";
        let blocks = vec![
            EditBlock {
                line_range: None,
                old: "aaa".into(),
                new: "AAA".into(),
            },
            EditBlock {
                line_range: None,
                old: "ccc".into(),
                new: "CCC".into(),
            },
        ];
        let (out, n) = apply_edit_blocks(contents, &blocks).unwrap();
        assert_eq!(out, "AAA\nbbb\nCCC");
        assert_eq!(n, 2);
    }

    #[test]
    fn apply_two_blocks_aborts_if_second_fails() {
        let contents = "aaa\nbbb";
        let blocks = vec![
            EditBlock {
                line_range: None,
                old: "aaa".into(),
                new: "AAA".into(),
            },
            EditBlock {
                line_range: None,
                old: "nope".into(),
                new: "X".into(),
            },
        ];
        let result = apply_edit_blocks(contents, &blocks);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("block 2"));
    }

    #[test]
    fn apply_line_range_scope() {
        let contents = "fn foo() {}\nfn bar() {}\nfn foo() {}";
        let blocks = vec![EditBlock {
            line_range: Some((1, 1)),
            old: "fn foo()".into(),
            new: "fn FOO()".into(),
        }];
        let (out, _) = apply_edit_blocks(contents, &blocks).unwrap();
        assert_eq!(out, "fn FOO() {}\nfn bar() {}\nfn foo() {}");
    }

    #[test]
    fn apply_line_range_out_of_bounds() {
        let contents = "a\nb";
        let blocks = vec![EditBlock {
            line_range: Some((10, 20)),
            old: "a".into(),
            new: "A".into(),
        }];
        let err = apply_edit_blocks(contents, &blocks).unwrap_err();
        assert!(err.contains("out of bounds"));
    }

    #[test]
    fn apply_fuzzy_match_whitespace_drift() {
        // File on disk has tab indentation; the model remembered 4 spaces.
        let contents = "fn foo() {\n\tlet x = 1;\n}\n";
        let blocks = vec![EditBlock {
            line_range: None,
            old: "fn foo() {\n    let x = 1;\n}".into(),
            new: "fn foo() {\n    let x = 42;\n}".into(),
        }];
        let (out, _) = apply_edit_blocks(contents, &blocks).unwrap();
        assert_eq!(out, "fn foo() {\n    let x = 42;\n}\n");
    }

    #[test]
    fn apply_fuzzy_match_non_unique() {
        let contents = "fn foo() {\n  let x = 1;\n}\nfn foo() {\n\tlet x = 1;\n}\n";
        let blocks = vec![EditBlock {
            line_range: None,
            old: "fn foo() {\n    let x = 1;\n}".into(),
            new: "X".into(),
        }];
        let err = apply_edit_blocks(contents, &blocks).unwrap_err();
        assert!(err.contains("fuzzy match found"));
    }

    #[test]
    fn apply_exact_match_multiple_still_errors() {
        // Two exact matches → existing error (not fuzzy).
        let contents = "aaa bbb aaa";
        let blocks = vec![EditBlock {
            line_range: None,
            old: "aaa".into(),
            new: "ccc".into(),
        }];
        let err = apply_edit_blocks(contents, &blocks).unwrap_err();
        assert!(err.contains("found 2 times"));
    }

    #[test]
    fn line_range_bytes_first_line() {
        assert_eq!(line_range_bytes("abc\ndef\nghi", 1, 1), Some((0, 3)));
    }

    #[test]
    fn line_range_bytes_middle_line() {
        assert_eq!(line_range_bytes("abc\ndef\nghi", 2, 2), Some((4, 7)));
    }

    #[test]
    fn line_range_bytes_range() {
        assert_eq!(line_range_bytes("abc\ndef\nghi", 1, 2), Some((0, 7)));
    }

    #[test]
    fn line_range_bytes_past_eof() {
        assert!(line_range_bytes("abc\ndef", 5, 5).is_none());
    }
}
