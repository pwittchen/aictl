use std::fmt::Write as _;

use crate::config::MAX_TOOL_OUTPUT_LEN;

use super::util::truncate_output;

pub(super) async fn tool_read_file(input: &str) -> String {
    let path = input.trim();
    match tokio::fs::read_to_string(path).await {
        Ok(mut contents) => {
            if contents.is_empty() {
                contents = "(empty file)".to_string();
            }
            truncate_output(&mut contents);
            contents
        }
        Err(e) => format!("Error reading file: {e}"),
    }
}

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

pub(super) async fn tool_search_files(input: &str) -> String {
    let input = input.trim();
    let (pattern, dir) = match input.split_once('\n') {
        Some((p, d)) => (p.trim(), d.trim()),
        None => (input, "."),
    };
    let dir = if dir.is_empty() { "." } else { dir };
    let pattern = pattern.to_string();
    let dir = dir.to_string();
    tokio::task::spawn_blocking(move || search_files_blocking(&pattern, &dir))
        .await
        .unwrap_or_else(|e| format!("Error running search: {e}"))
}

fn search_files_blocking(pattern: &str, dir: &str) -> String {
    let glob_pattern = format!("{dir}/**/*");
    let entries = match glob::glob(&glob_pattern) {
        Ok(paths) => paths,
        Err(e) => return format!("Error: invalid path pattern: {e}"),
    };
    let mut result = String::new();
    for entry in entries {
        let Ok(path) = entry else { continue };
        if !path.is_file() {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue; // skip binary / unreadable files
        };
        let path_str = path.to_string_lossy();
        for (i, line) in contents.lines().enumerate() {
            if line.contains(pattern) {
                if !result.is_empty() {
                    result.push('\n');
                }
                let _ = write!(result, "{path_str}:{}:{line}", i + 1);
                if result.len() > MAX_TOOL_OUTPUT_LEN {
                    result.truncate(MAX_TOOL_OUTPUT_LEN);
                    result.push_str("\n... (truncated)");
                    return result;
                }
            }
        }
    }
    if result.is_empty() {
        "No matches found.".to_string()
    } else {
        result
    }
}

pub(super) async fn tool_edit_file(input: &str) -> String {
    let input = input.trim();
    // Parse: path\n<<<\nold\n===\nnew\n>>>
    let Some((path, rest)) = input.split_once('\n') else {
        return "Invalid input: expected file path on first line".to_string();
    };
    let path = path.trim();
    let rest = rest.trim();
    let Some(rest) = rest.strip_prefix("<<<") else {
        return "Invalid input: expected <<< delimiter after file path".to_string();
    };
    let Some((old_new, _)) = rest.split_once(">>>") else {
        return "Invalid input: expected >>> closing delimiter".to_string();
    };
    let Some((old_text, new_text)) = old_new.split_once("===") else {
        return "Invalid input: expected === separator between old and new text".to_string();
    };
    let old_text = old_text.strip_prefix('\n').unwrap_or(old_text);
    let old_text = old_text.strip_suffix('\n').unwrap_or(old_text);
    let new_text = new_text.strip_prefix('\n').unwrap_or(new_text);
    let new_text = new_text.strip_suffix('\n').unwrap_or(new_text);

    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading file: {e}"),
    };
    let count = contents.matches(old_text).count();
    if count == 0 {
        return "Error: old text not found in file".to_string();
    }
    if count > 1 {
        return format!(
            "Error: old text found {count} times in file — provide more context to match uniquely"
        );
    }
    let updated = contents.replacen(old_text, new_text, 1);
    match tokio::fs::write(path, &updated).await {
        Ok(()) => format!("Edited {path} (replaced 1 occurrence)"),
        Err(e) => format!("Error writing file: {e}"),
    }
}

pub(super) fn tool_find_files(input: &str) -> String {
    let input = input.trim();
    let (pattern, base_dir) = match input.split_once('\n') {
        Some((p, d)) => (p.trim(), d.trim()),
        None => (input, "."),
    };
    let base_dir = if base_dir.is_empty() { "." } else { base_dir };
    let full_pattern = if std::path::Path::new(pattern).is_absolute() {
        pattern.to_string()
    } else {
        format!("{base_dir}/{pattern}")
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
                    result.truncate(MAX_TOOL_OUTPUT_LEN);
                    result.push_str("\n... (truncated)");
                    break;
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
