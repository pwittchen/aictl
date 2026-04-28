//! Tool-call parsing, dispatch, and execution.
//!
//! The model emits tool invocations as `<tool name="...">...</tool>` XML tags;
//! [`parse_tool_call`] extracts them, [`execute_tool`] dispatches to the
//! appropriate submodule implementation after running security and
//! duplicate-call guards, and [`looks_like_malformed_tool_call`] tells the
//! agent loop when to ask the model to retry a broken tag.
//!
//! Each tool lives in its own submodule so this file stays focused on the
//! dispatch/parse surface. Submodules expose `pub(super)` async functions that
//! return plain `String` (or [`ToolOutput`] for `read_image`, which carries
//! image bytes alongside text).

use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::ImageData;

mod archive;
mod calculate;
mod check_port;
mod checksum;
mod clipboard;
mod csv_query;
mod datetime;
mod diff;
mod document;
mod filesystem;
mod geo;
mod git;
mod image;
mod json_query;
mod lint;
mod list_processes;
mod notify;
mod run_code;
mod shell;
mod system_info;
mod util;
mod web;

/// Slot holding the most recent successfully dispatched tool invocation,
/// keyed by `(tool_name, normalized_input)`. Used to block the model from
/// calling the same tool with the same input value *back-to-back* —
/// weaker models (e.g. small local GGUFs) otherwise loop indefinitely,
/// re-running the same search or fetch. Only consecutive repeats are
/// blocked: any intervening tool call (or new user/assistant turn that
/// clears the slot) lets the same call run again, so legitimate
/// re-reads (`read_file` → `edit_file` → `read_file` to verify) are
/// not penalized.
static LAST_CALL: OnceLock<Mutex<Option<(String, String)>>> = OnceLock::new();

fn last_call() -> &'static Mutex<Option<(String, String)>> {
    LAST_CALL.get_or_init(|| Mutex::new(None))
}

/// Normalize tool input for duplicate detection: lowercase, strip
/// punctuation, collapse whitespace. Trivial formatting differences
/// ("Weather, Gliwice?" vs "weather gliwice") therefore collide.
fn normalize_input(input: &str) -> String {
    input
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalize an MCP tool body (JSON object) so that whitespace differences
/// don't make the duplicate-call guard treat semantically identical calls as
/// distinct. Falls back to the generic [`normalize_input`] if the body isn't
/// valid JSON — the gate still works, it's just less robust.
fn normalize_mcp_input(input: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(input.trim()) {
        Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| input.to_string()),
        Err(_) => normalize_input(input),
    }
}

/// Clear the last-call slot. Called on REPL `/clear`, session switches,
/// and at the start of every new user turn so a new conversation (or a
/// new user message after a final answer) starts with a blank slate.
pub fn clear_call_history() {
    if let Ok(mut slot) = last_call().lock() {
        *slot = None;
    }
}

/// Returns `true` if this tool call (same name, same normalized input)
/// is identical to the most recent one — i.e. the model is trying to
/// repeat itself back-to-back. Does not mutate the slot — used by the
/// agent loop to abort *before* spending another LLM round-trip on a
/// call that would be rejected anyway.
pub fn is_duplicate_call(tool_call: &ToolCall) -> bool {
    let key = (tool_call.name.clone(), normalize_for(&tool_call.name, &tool_call.input));
    last_call()
        .lock()
        .map(|slot| slot.as_ref() == Some(&key))
        .unwrap_or(false)
}

/// Pick the right normalizer for a tool name. MCP tool bodies are JSON
/// objects, so canonicalize them before keying the duplicate slot.
fn normalize_for(tool_name: &str, input: &str) -> String {
    if tool_name.starts_with("mcp__") {
        normalize_mcp_input(input)
    } else {
        normalize_input(input)
    }
}

#[derive(Debug)]
pub struct ToolCall {
    pub name: String,
    pub input: String,
}

/// Result of executing a tool: text output plus optional image data.
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<ImageData>,
}

impl ToolOutput {
    fn text(s: String) -> Self {
        Self {
            text: s,
            images: vec![],
        }
    }
}

pub const TOOL_COUNT: usize = 31;

pub fn parse_tool_call(response: &str) -> Option<ToolCall> {
    let start_prefix = "<tool name=\"";
    let start_idx = response.find(start_prefix)?;
    let after_prefix = start_idx + start_prefix.len();
    let name_end = response[after_prefix..].find('"')?;
    let name = response[after_prefix..after_prefix + name_end].to_string();
    let tag_close = response[after_prefix + name_end..].find('>')?;
    let content_start = after_prefix + name_end + tag_close + 1;
    let end_tag = "</tool>";
    let content_end = response[content_start..].find(end_tag)?;
    let input = response[content_start..content_start + content_end]
        .trim()
        .to_string();
    Some(ToolCall { name, input })
}

/// Returns `true` when the response clearly *attempted* a tool call but
/// [`parse_tool_call`] couldn't extract one — i.e. the `<tool>` XML is
/// malformed (missing close tag, wrong quote style, broken attribute, ...).
///
/// The agent loop uses this to ask the model to retry instead of surfacing
/// raw tool markup to the user as a "final answer".
pub fn looks_like_malformed_tool_call(response: &str) -> bool {
    if parse_tool_call(response).is_some() {
        return false;
    }
    // Strong signal: the exact prefix we parse is present but something
    // after it is broken (e.g. missing `"`, `>`, or `</tool>`).
    if response.contains("<tool name=") {
        return true;
    }
    // Also catch cases where both a tag-opener and a closer appear but the
    // name attribute uses the wrong quoting style or other variants.
    let has_open = response.contains("<tool>") || response.contains("<tool ");
    let has_close = response.contains("</tool>");
    has_open && has_close
}

/// Check whether tools are globally enabled via `AICTL_TOOLS_ENABLED` config.
/// Returns `true` when the key is absent or set to anything other than `false`/`0`.
pub fn tools_enabled() -> bool {
    crate::config::config_get("AICTL_TOOLS_ENABLED").is_none_or(|v| v != "false" && v != "0")
}

pub async fn execute_tool(tool_call: &ToolCall) -> ToolOutput {
    // Global tools switch
    if !tools_enabled() {
        crate::audit::log_tool(tool_call, crate::audit::Outcome::DisabledGlobally);
        return ToolOutput::text(
            "All tools are disabled (AICTL_TOOLS_ENABLED=false in config)".to_string(),
        );
    }

    // Duplicate-call guard: refuse to run the same tool with the same
    // (normalized) input *back-to-back*. Only consecutive repeats are
    // blocked — any intervening tool call (or a new user/assistant turn
    // that clears the slot) lets the same call run again, so legitimate
    // re-reads aren't penalized. The model gets a clear message instead
    // of a fresh result, which breaks the tool-call loops that weaker
    // models otherwise enter.
    let call_key = (
        tool_call.name.clone(),
        normalize_for(&tool_call.name, &tool_call.input),
    );
    {
        let mut slot = last_call().lock().expect("tool call slot poisoned");
        if slot.as_ref() == Some(&call_key) {
            crate::audit::log_tool(tool_call, crate::audit::Outcome::DuplicateCall);
            return ToolOutput::text(format!(
                "You just called the tool `{}` with this input back-to-back, and its result is already in the conversation right above. Do not repeat the same tool call. Answer now with your final response based on the information you already have, or call a different tool with a meaningfully different input.",
                tool_call.name
            ));
        }
        *slot = Some(call_key);
    }

    // Security gate
    if let Err(reason) = crate::security::validate_tool(tool_call) {
        crate::audit::log_tool(
            tool_call,
            crate::audit::Outcome::DeniedByPolicy { reason: &reason },
        );
        return ToolOutput::text(format!("Security policy denied: {reason}"));
    }

    let input = &tool_call.input;

    // read_image returns ToolOutput with image data
    if tool_call.name == "read_image" {
        let mut output = image::tool_read_image(input).await;
        output.text = crate::security::sanitize_output(&output.text);
        crate::audit::log_tool(
            tool_call,
            crate::audit::Outcome::Executed {
                result: &output.text,
            },
        );
        return output;
    }

    let result = match tool_call.name.as_str() {
        "exec_shell" => shell::tool_exec_shell(input).await,
        "read_file" => filesystem::tool_read_file(input).await,
        "write_file" => filesystem::tool_write_file(input).await,
        "remove_file" => filesystem::tool_remove_file(input).await,
        "create_directory" => filesystem::tool_create_directory(input).await,
        "list_directory" => filesystem::tool_list_directory(input).await,
        "search_files" => filesystem::tool_search_files(input).await,
        "edit_file" => filesystem::tool_edit_file(input).await,
        "diff_files" => diff::tool_diff_files(input).await,
        "search_web" => web::tool_search_web(input).await,
        "find_files" => filesystem::tool_find_files(input),
        "fetch_url" => web::tool_fetch_url(input).await,
        "extract_website" => web::tool_extract_website(input).await,
        "fetch_datetime" => datetime::tool_fetch_datetime().await,
        "fetch_geolocation" => geo::tool_fetch_geolocation(input).await,
        "generate_image" => image::tool_generate_image(input).await,
        "read_document" => document::tool_read_document(input).await,
        "git" => git::tool_git(input).await,
        "run_code" => run_code::tool_run_code(input).await,
        "lint_file" => lint::tool_lint_file(input).await,
        "json_query" => json_query::tool_json_query(input).await,
        "csv_query" => csv_query::tool_csv_query(input).await,
        "calculate" => calculate::tool_calculate(input),
        "list_processes" => list_processes::tool_list_processes(input).await,
        "check_port" => check_port::tool_check_port(input).await,
        "system_info" => system_info::tool_system_info(input).await,
        "archive" => archive::tool_archive(input).await,
        "checksum" => checksum::tool_checksum(input).await,
        "clipboard" => clipboard::tool_clipboard(input).await,
        "notify" => notify::tool_notify(input).await,
        other if other.starts_with("mcp__") => crate::mcp::call_tool(other, input).await,
        other => {
            if let Some(plugin) = crate::plugins::find(other) {
                crate::plugins::execute_plugin(plugin, input).await
            } else {
                format!("Unknown tool: {other}")
            }
        }
    };
    let sanitized = crate::security::sanitize_output(&result);
    crate::audit::log_tool(
        tool_call,
        crate::audit::Outcome::Executed { result: &sanitized },
    );
    ToolOutput::text(sanitized)
}

pub fn confirm_tool_call(tool_call: &ToolCall) -> bool {
    eprint!(
        "Tool call [{}]: {}\nAllow? [y/N] ",
        tool_call.name, tool_call.input
    );
    std::io::stderr().flush().ok();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim(), "y" | "Y" | "yes" | "Yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_simple() {
        let resp = r#"<tool name="read_file">src/main.rs</tool>"#;
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "read_file");
        assert_eq!(tc.input, "src/main.rs");
    }

    #[test]
    fn parse_valid_multiline_input() {
        let resp = "<tool name=\"write_file\">\npath/to/file\nline one\nline two\n</tool>";
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "write_file");
        assert_eq!(tc.input, "path/to/file\nline one\nline two");
    }

    #[test]
    fn parse_extra_text_around_tags() {
        let resp = "Let me read that file for you.\n<tool name=\"read_file\">foo.txt</tool>\nDone.";
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "read_file");
        assert_eq!(tc.input, "foo.txt");
    }

    #[test]
    fn parse_missing_closing_tag() {
        let resp = r#"<tool name="exec_shell">ls -la"#;
        assert!(parse_tool_call(resp).is_none());
    }

    #[test]
    fn parse_missing_opening_tag() {
        let resp = "some text</tool>";
        assert!(parse_tool_call(resp).is_none());
    }

    #[test]
    fn parse_empty_input_between_tags() {
        let resp = r#"<tool name="fetch_datetime"></tool>"#;
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "fetch_datetime");
        assert_eq!(tc.input, "");
    }

    #[test]
    fn parse_tool_name_with_underscore() {
        let resp = r#"<tool name="search_files">pattern</tool>"#;
        let tc = parse_tool_call(resp).unwrap();
        assert_eq!(tc.name, "search_files");
    }

    #[test]
    fn parse_no_tool_call_plain_text() {
        let resp = "Here is the answer to your question.";
        assert!(parse_tool_call(resp).is_none());
    }

    #[test]
    fn parse_incomplete_opening_tag() {
        let resp = r#"<tool name="exec_shell"#;
        assert!(parse_tool_call(resp).is_none());
    }

    // --- Malformed tool call detection ---

    #[test]
    fn malformed_detects_missing_closing_tag() {
        // LLM wrote a tool call but forgot `</tool>` — regression test for bug
        // where this was surfaced to the user as a raw-XML "final answer".
        let resp = r#"I'll read that file.
<tool name="read_file">src/main.rs"#;
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_unterminated_name_attribute() {
        let resp = r#"<tool name="exec_shell>ls -la</tool>"#;
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_single_quoted_name() {
        // Wrong quote style — parser expects double quotes.
        let resp = "<tool name='read_file'>foo.txt</tool>";
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_truncated_opening_tag() {
        let resp = r#"<tool name="exec_shell"#;
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_detects_bare_tool_tags_without_name_attr() {
        let resp = "<tool>read_file src/main.rs</tool>";
        assert!(parse_tool_call(resp).is_none());
        assert!(looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_valid_tool_call() {
        let resp = r#"<tool name="read_file">src/main.rs</tool>"#;
        assert!(parse_tool_call(resp).is_some());
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_plain_text_answer() {
        let resp = "Here is the answer to your question. It is 42.";
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_answer_mentioning_tool_word() {
        // The word "toolchain" must not trip the heuristic.
        let resp = "You can install it via the standard Rust toolchain.";
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_answer_with_only_closing_tag_mention() {
        // A final answer that happens to mention the closing tag textually
        // (no `<tool` open anywhere) must not trigger retry.
        let resp = "The closing XML marker is </tool> — that's how it ends.";
        assert!(!looks_like_malformed_tool_call(resp));
    }

    #[test]
    fn malformed_rejects_valid_call_with_leading_text() {
        let resp = "Sure, let me check.\n<tool name=\"read_file\">a.txt</tool>\nDone.";
        assert!(parse_tool_call(resp).is_some());
        assert!(!looks_like_malformed_tool_call(resp));
    }

    // --- Duplicate-call guard tests ---

    /// Verify the slot blocks only consecutive identical calls. We hold
    /// the slot mutex for the whole test so parallel `execute_tool`
    /// callers in other tests can't clobber the state mid-assertion;
    /// that means we can't go through `is_duplicate_call` /
    /// `clear_call_history` here (they'd re-lock and deadlock), so we
    /// assert against the slot directly — which is exactly what those
    /// public functions observe.
    #[test]
    fn duplicate_guard_blocks_only_consecutive_repeats() {
        let mut slot = last_call().lock().expect("slot poisoned");
        let key_a = ("read_file".to_string(), normalize_input("/tmp/a.txt"));
        let key_b = ("read_file".to_string(), normalize_input("/tmp/b.txt"));

        // Empty slot (fresh session / after clear): nothing is a duplicate.
        *slot = None;
        assert_ne!(slot.as_ref(), Some(&key_a));

        // First dispatch of A populates the slot — an immediate repeat
        // would now hit.
        *slot = Some(key_a.clone());
        assert_eq!(slot.as_ref(), Some(&key_a));

        // A *different* call (B) takes the slot, so a follow-up A is no
        // longer back-to-back and must be allowed through.
        *slot = Some(key_b.clone());
        assert_ne!(slot.as_ref(), Some(&key_a));

        // Same call reasserts the duplicate state.
        *slot = Some(key_b.clone());
        assert_eq!(slot.as_ref(), Some(&key_b));

        // clear_call_history-equivalent: slot back to None, nothing
        // duplicates.
        *slot = None;
        assert_ne!(slot.as_ref(), Some(&key_a));
        assert_ne!(slot.as_ref(), Some(&key_b));
    }

    // --- Tool execution tests ---

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aictl_test_{name}_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn exec_read_file() {
        let dir = tmp_dir("read");
        let path = dir.join("hello.txt");
        std::fs::write(&path, "hello world").unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_file".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert_eq!(result.text, "hello world");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_read_file_empty() {
        let dir = tmp_dir("read_empty");
        let path = dir.join("empty.txt");
        std::fs::write(&path, "").unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_file".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert_eq!(result.text, "(empty file)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_read_file_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_file".into(),
            input: "/tmp/aictl_nonexistent_file_xyz".into(),
        })
        .await;
        assert!(result.text.starts_with("Error reading file:"));
    }

    #[tokio::test]
    async fn exec_write_file() {
        let dir = tmp_dir("write");
        let path = dir.join("out.txt");
        let input = format!("{}\nfile content here", path.display());
        let result = execute_tool(&ToolCall {
            name: "write_file".into(),
            input,
        })
        .await;
        assert!(result.text.starts_with("Wrote"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "file content here");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_write_file_no_newline() {
        let result = execute_tool(&ToolCall {
            name: "write_file".into(),
            input: "single_line_no_newline".into(),
        })
        .await;
        assert!(result.text.contains("Invalid input"));
    }

    #[tokio::test]
    async fn exec_remove_file() {
        let dir = tmp_dir("remove");
        let path = dir.join("deleteme.txt");
        std::fs::write(&path, "gone soon").unwrap();
        assert!(path.exists());
        let result = execute_tool(&ToolCall {
            name: "remove_file".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.starts_with("Removed"));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_remove_file_not_found() {
        let result = execute_tool(&ToolCall {
            name: "remove_file".into(),
            input: "/tmp/aictl_nonexistent_file_xyz".into(),
        })
        .await;
        assert!(result.text.starts_with("Error removing file:"));
    }

    #[tokio::test]
    async fn exec_create_directory() {
        let dir = tmp_dir("create_dir");
        let new_dir = dir.join("a/b/c");
        assert!(!new_dir.exists());
        let result = execute_tool(&ToolCall {
            name: "create_directory".into(),
            input: new_dir.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.starts_with("Created directory"));
        assert!(new_dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_list_directory() {
        let dir = tmp_dir("listdir");
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        let result = execute_tool(&ToolCall {
            name: "list_directory".into(),
            input: dir.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.contains("[FILE]"));
        assert!(result.text.contains("[DIR]"));
        assert!(result.text.contains("a.txt"));
        assert!(result.text.contains("subdir"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_list_directory_empty() {
        let dir = tmp_dir("listdir_empty");
        let result = execute_tool(&ToolCall {
            name: "list_directory".into(),
            input: dir.to_string_lossy().into(),
        })
        .await;
        assert_eq!(result.text, "(empty directory)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_edit_file_success() {
        let dir = tmp_dir("edit_ok");
        let path = dir.join("file.txt");
        std::fs::write(&path, "hello world").unwrap();
        let input = format!("{}\n<<<\nhello\n===\ngoodbye\n>>>", path.display());
        let result = execute_tool(&ToolCall {
            name: "edit_file".into(),
            input,
        })
        .await;
        assert!(result.text.contains("replaced 1 occurrence"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye world");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_edit_file_not_found() {
        let dir = tmp_dir("edit_nf");
        let path = dir.join("file.txt");
        std::fs::write(&path, "hello world").unwrap();
        let input = format!(
            "{}\n<<<\nno such text\n===\nreplacement\n>>>",
            path.display()
        );
        let result = execute_tool(&ToolCall {
            name: "edit_file".into(),
            input,
        })
        .await;
        assert!(result.text.contains("old text not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_edit_file_multiple() {
        let dir = tmp_dir("edit_multi");
        let path = dir.join("file.txt");
        std::fs::write(&path, "aaa bbb aaa").unwrap();
        let input = format!("{}\n<<<\naaa\n===\nccc\n>>>", path.display());
        let result = execute_tool(&ToolCall {
            name: "edit_file".into(),
            input,
        })
        .await;
        assert!(result.text.contains("found 2 times"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_find_files() {
        let dir = tmp_dir("find");
        std::fs::write(dir.join("a.rs"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        let input = format!("*.rs\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "find_files".into(),
            input,
        })
        .await;
        assert!(result.text.contains("a.rs"));
        assert!(!result.text.contains("b.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_find_files_no_matches() {
        let dir = tmp_dir("find_none");
        let input = format!("*.xyz\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "find_files".into(),
            input,
        })
        .await;
        assert_eq!(result.text, "No matches found.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_search_files() {
        let dir = tmp_dir("search");
        std::fs::write(dir.join("match.txt"), "needle in haystack").unwrap();
        std::fs::write(dir.join("other.txt"), "nothing here").unwrap();
        let input = format!("needle\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "search_files".into(),
            input,
        })
        .await;
        assert!(result.text.contains("match.txt"));
        assert!(result.text.contains("needle in haystack"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_search_files_no_matches() {
        let dir = tmp_dir("search_none");
        std::fs::write(dir.join("file.txt"), "hello").unwrap();
        let input = format!("zzzzz\n{}", dir.display());
        let result = execute_tool(&ToolCall {
            name: "search_files".into(),
            input,
        })
        .await;
        assert_eq!(result.text, "No matches found.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_shell_stdout() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "echo hello".into(),
        })
        .await;
        assert_eq!(result.text.trim(), "hello");
    }

    #[tokio::test]
    async fn exec_shell_stderr() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "echo oops >&2".into(),
        })
        .await;
        assert!(result.text.contains("[stderr]"));
        assert!(result.text.contains("oops"));
    }

    #[tokio::test]
    async fn exec_shell_no_output() {
        let result = execute_tool(&ToolCall {
            name: "exec_shell".into(),
            input: "true".into(),
        })
        .await;
        assert_eq!(result.text, "(no output)");
    }

    #[tokio::test]
    async fn exec_fetch_datetime() {
        let result = execute_tool(&ToolCall {
            name: "fetch_datetime".into(),
            input: String::new(),
        })
        .await;
        assert!(!result.text.is_empty());
        assert!(result.text.starts_with("20"));
    }

    #[test]
    fn normalize_mcp_input_canonicalizes_json() {
        // Same JSON with different whitespace must collapse to the same key
        // so the duplicate-call guard doesn't treat them as distinct.
        let a = normalize_mcp_input(r#"{"a":1,"b":"x"}"#);
        let b = normalize_mcp_input(r#"{ "a" : 1 , "b" : "x" }"#);
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_mcp_input_falls_back_for_non_json() {
        // Garbage text falls through to the generic normalizer rather than
        // panicking — keeps the gate working even for malformed bodies.
        let out = normalize_mcp_input("not  json   here");
        assert_eq!(out, "not json here");
    }

    #[test]
    fn normalize_for_routes_mcp_names_to_json_canonicalizer() {
        let a = normalize_for("mcp__fs__read", r#"{"path":"/a"}"#);
        let b = normalize_for("mcp__fs__read", r#"{ "path" : "/a" }"#);
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn exec_unknown_tool() {
        let result = execute_tool(&ToolCall {
            name: "nonexistent".into(),
            input: String::new(),
        })
        .await;
        assert_eq!(result.text, "Unknown tool: nonexistent");
    }

    #[tokio::test]
    async fn exec_read_image_file() {
        let dir = tmp_dir("read_img");
        let path = dir.join("test.png");
        // Write a minimal valid PNG (1x1 pixel, white)
        let png_bytes: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
            0x77, 0x53, 0xDE,
        ];
        std::fs::write(&path, png_bytes).unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_image".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.contains("Image loaded"));
        assert!(result.text.contains("image/png"));
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].media_type, "image/png");
        assert!(!result.images[0].base64_data.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn exec_read_image_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_image".into(),
            input: "/tmp/aictl_nonexistent_image.png".into(),
        })
        .await;
        assert!(result.text.starts_with("Error reading image file:"));
        assert!(result.images.is_empty());
    }

    #[tokio::test]
    async fn exec_read_image_empty_input() {
        let result = execute_tool(&ToolCall {
            name: "read_image".into(),
            input: String::new(),
        })
        .await;
        assert!(result.text.contains("no file path or URL"));
        assert!(result.images.is_empty());
    }

    // --- read_document tests ---

    #[tokio::test]
    async fn exec_read_document_empty_input() {
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: String::new(),
        })
        .await;
        assert!(result.text.contains("no file path"));
    }

    #[tokio::test]
    async fn exec_read_document_unsupported_format() {
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: "file.txt".into(),
        })
        .await;
        assert!(result.text.contains("unsupported document format"));
        assert!(result.text.contains(".txt"));
    }

    #[tokio::test]
    async fn exec_read_document_pdf_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: "/tmp/aictl_nonexistent.pdf".into(),
        })
        .await;
        assert!(result.text.starts_with("Error reading file:"));
    }

    #[tokio::test]
    async fn exec_read_document_docx_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: "/tmp/aictl_nonexistent.docx".into(),
        })
        .await;
        assert!(result.text.starts_with("Error reading file:"));
    }

    #[tokio::test]
    async fn exec_read_document_invalid_docx() {
        let dir = tmp_dir("bad_docx");
        let path = dir.join("bad.docx");
        std::fs::write(&path, "not a zip file").unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.contains("Error reading DOCX archive"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- spreadsheet (read_document with .xlsx/.xls/.ods) tests ---

    #[tokio::test]
    async fn exec_read_document_unsupported_zzz() {
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: "file.zzz".into(),
        })
        .await;
        assert!(result.text.contains("unsupported document format"));
        assert!(result.text.contains(".xlsx"));
    }

    #[tokio::test]
    async fn exec_read_document_xlsx_not_found() {
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: "/tmp/aictl_nonexistent.xlsx".into(),
        })
        .await;
        assert!(result.text.contains("Error opening spreadsheet"));
    }

    #[tokio::test]
    async fn exec_read_document_invalid_xlsx() {
        let dir = tmp_dir("bad_xlsx");
        let path = dir.join("bad.xlsx");
        std::fs::write(&path, "not a valid xlsx").unwrap();
        let result = execute_tool(&ToolCall {
            name: "read_document".into(),
            input: path.to_string_lossy().into(),
        })
        .await;
        assert!(result.text.contains("Error opening spreadsheet"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
