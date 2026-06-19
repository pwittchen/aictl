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
mod draw_chart;
mod filesystem;
mod geo;
mod git;
mod image;
mod json_query;
mod lint;
mod list_processes;
mod memory;
mod notify;
mod run_code;
mod shell;
mod system_info;
mod test;
mod util;
mod view_map;
mod web;

pub(crate) use lint::tool_lint_file;
pub use test::{TestFailure, TestSummary, take_last_summary as take_last_test_summary};

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
    let key = (
        tool_call.name.clone(),
        normalize_for(&tool_call.name, &tool_call.input),
    );
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

#[derive(Debug, Clone)]
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

pub const TOOL_COUNT: usize = 36;

/// `(name, one-line description)` for every built-in tool. Both the CLI's
/// `/tools` printer and the desktop Settings panel render from this single
/// source — a CI test in `aictl-cli` keeps the length in sync with
/// [`TOOL_COUNT`].
pub const BUILTIN_TOOLS: &[(&str, &str)] = &[
    ("exec_shell", "execute a shell command via sh -c"),
    (
        "read_file",
        "read a file; optional --lines [N|N-M] for slice and numbered output",
    ),
    ("write_file", "write content to a file"),
    ("remove_file", "remove (delete) a file"),
    (
        "edit_file",
        "edit a file with multi-block find-and-replace; optional @start-end line scope and fuzzy fallback",
    ),
    (
        "diff_files",
        "compute a unified diff between two text files",
    ),
    (
        "create_directory",
        "create a directory and any missing parents",
    ),
    ("list_directory", "list files and directories at a path"),
    (
        "search_files",
        "search file contents (ripgrep when available); --regex / --type / --context / --case",
    ),
    (
        "find_files",
        "find files by glob; --type for fast language filter (ripgrep when available)",
    ),
    (
        "search_web_fc",
        "search the web via Firecrawl API (primary)",
    ),
    (
        "search_web_ddg",
        "search the web via DuckDuckGo (fallback, no key)",
    ),
    ("fetch_url", "fetch a URL and return text content"),
    ("extract_website", "extract readable content from a URL"),
    ("fetch_datetime", "get current date, time, and timezone"),
    (
        "fetch_geolocation",
        "get geolocation data for an IP address",
    ),
    (
        "view_map",
        "display a map (OpenStreetMap) — desktop app only",
    ),
    ("draw_chart", "render a chart (Chart.js) — desktop app only"),
    ("read_image", "read an image from file or URL for analysis"),
    (
        "generate_image",
        "generate an image from text (GPT Image/Imagen/Grok)",
    ),
    (
        "read_document",
        "read a PDF, DOCX, or spreadsheet as markdown",
    ),
    (
        "git",
        "run a safe git subcommand (status/diff/log/blame/commit)",
    ),
    (
        "run_code",
        "execute a snippet (python/node/ruby/perl/lua/bash/sh)",
    ),
    (
        "lint_file",
        "run a language-appropriate linter/formatter on a file",
    ),
    (
        "test",
        "run the project's test command and return structured pass/fail counts",
    ),
    (
        "json_query",
        "query/transform JSON with jq-like expressions",
    ),
    (
        "csv_query",
        "filter CSV/TSV with SQL-like expressions (table output)",
    ),
    ("calculate", "evaluate a math expression safely"),
    (
        "list_processes",
        "list running processes with structured filtering",
    ),
    ("check_port", "test TCP reachability of a host:port"),
    (
        "system_info",
        "OS/CPU/memory/disk info as markdown (cross-platform)",
    ),
    (
        "archive",
        "create, extract, or list tar.gz/tgz/tar/zip archives",
    ),
    (
        "checksum",
        "compute SHA-256 and/or MD5 of a file (streaming)",
    ),
    ("clipboard", "read or write the system clipboard"),
    ("notify", "send a desktop notification (macOS, Linux)"),
    (
        "save_memory",
        "persist a fact about the user to long-term memory",
    ),
];

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

/// Parse every well-formed `<tool …>…</tool>` block from a model response, in
/// source order.
///
/// Phase 4 batch-dispatch entry point: when the model emits more than one
/// `<tool>` block in a single response (legal only for read-only calls — see
/// [`is_parallelizable`]), the agent loop uses this instead of
/// [`parse_tool_call`] so it can dispatch the batch in parallel.
///
/// Single-call shape is preserved: any response that returns `Some(_)` from
/// [`parse_tool_call`] returns a `Vec` of length 1 here. Malformed tags are
/// silently skipped — the existing [`looks_like_malformed_tool_call`] helper
/// handles "model tried but produced invalid XML" when this returns empty.
#[must_use]
pub fn parse_tool_calls(response: &str) -> Vec<ToolCall> {
    let start_prefix = "<tool name=\"";
    let end_tag = "</tool>";
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < response.len() {
        let Some(start_idx) = response[cursor..].find(start_prefix) else {
            break;
        };
        let abs_start = cursor + start_idx;
        let after_prefix = abs_start + start_prefix.len();
        let Some(name_end) = response[after_prefix..].find('"') else {
            break;
        };
        let name = response[after_prefix..after_prefix + name_end].to_string();
        let Some(tag_close) = response[after_prefix + name_end..].find('>') else {
            break;
        };
        let content_start = after_prefix + name_end + tag_close + 1;
        let Some(content_end_rel) = response[content_start..].find(end_tag) else {
            break;
        };
        let input = response[content_start..content_start + content_end_rel]
            .trim()
            .to_string();
        out.push(ToolCall { name, input });
        cursor = content_start + content_end_rel + end_tag.len();
    }
    out
}

/// Tools whose execution mutates state outside the host's memory: file
/// writes, process spawns, network sends, persisted memory writes,
/// clipboard writes. These are *not* parallelizable — the model must
/// emit them alone, one per LLM response.
///
/// Note: `git` and `clipboard` are split — both have read-only and
/// side-effect modes; [`is_parallelizable`] inspects the body to
/// classify per call.
const SIDE_EFFECT_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "remove_file",
    "create_directory",
    "exec_shell",
    "run_code",
    "notify",
    "archive",
    "save_memory",
    "generate_image",
    "test",
];

/// `git` is split: status/log/blame/diff are read-only, commit is a
/// side-effect. The dispatch loop inspects the body's first token to
/// classify.
fn is_git_side_effect(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or("");
    matches!(first, "commit")
}

/// `clipboard` is split similarly — `read` is parallelizable, `write`
/// is a side-effect. Empty / unparseable body defaults to read.
fn is_clipboard_side_effect(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or("");
    matches!(first, "write")
}

/// Return `true` when this tool call is safe to run concurrently with
/// other parallelizable calls in the same batch.
///
/// Side-effect tools, MCP tools, and plugin tools always return `false`.
/// `git` and `clipboard` are split by body inspection.
#[must_use]
pub fn is_parallelizable(call: &ToolCall) -> bool {
    if SIDE_EFFECT_TOOLS.contains(&call.name.as_str()) {
        return false;
    }
    if call.name == "git" && is_git_side_effect(&call.input) {
        return false;
    }
    if call.name == "clipboard" && is_clipboard_side_effect(&call.input) {
        return false;
    }
    // MCP and plugin tools are conservatively *not* parallelizable in
    // v1. Their side-effect surface is unknown to us, so the safe
    // default is serial. A future MCP capability bit can lift this.
    if call.name.starts_with("mcp__") {
        return false;
    }
    if crate::plugins::find(&call.name).is_some() {
        return false;
    }
    true
}

/// Tools whose entire purpose is to surface an ambient, dynamic value the
/// model usually wants *before* it can form a meaningful request for another
/// tool: the caller's geolocation and the current date/time. When one of
/// these shares a batch with a [`CONTEXT_CONSUMER_TOOLS`] call, the consumer
/// almost certainly meant to use the produced value — but its input was
/// already committed as fixed text, so parallel dispatch would race the
/// producer and bake in stale or guessed context.
const CONTEXT_PRODUCER_TOOLS: &[&str] = &["fetch_geolocation", "fetch_datetime"];

/// Tools whose target/query the model composes itself and that send it over
/// the network — the place ambient context ("where am I", "what time is it")
/// gets embedded into a request. When batched alongside a
/// [`CONTEXT_PRODUCER_TOOLS`] call they must wait a turn so the model can
/// rewrite the request with the real value.
const CONTEXT_CONSUMER_TOOLS: &[&str] = &["fetch_url", "extract_website"];

/// `true` when this call produces an ambient value (location / time) that a
/// sibling in the same batch might depend on. See [`CONTEXT_PRODUCER_TOOLS`].
#[must_use]
pub fn is_context_producer(call: &ToolCall) -> bool {
    CONTEXT_PRODUCER_TOOLS.contains(&call.name.as_str())
}

/// `true` when this call composes a network request whose contents may depend
/// on a [`is_context_producer`] sibling. See [`CONTEXT_CONSUMER_TOOLS`].
#[must_use]
pub fn is_context_consumer(call: &ToolCall) -> bool {
    CONTEXT_CONSUMER_TOOLS.contains(&call.name.as_str())
}

/// How a read-only batch should be dispatched once data-dependencies are
/// accounted for.
#[derive(Debug)]
pub enum BatchPlan {
    /// No producer/consumer dependency — run every call in parallel as-is.
    Independent(Vec<ToolCall>),
    /// A context producer shares the batch with a consumer of its output.
    /// `run_now` (the producer(s) plus any calls independent of them) runs
    /// this turn; `deferred` (the consumers) is held back so the model can
    /// re-emit it next turn with the produced value in hand.
    Dependency {
        run_now: Vec<ToolCall>,
        deferred: Vec<ToolCall>,
    },
}

/// Split a read-only batch so data-dependent calls run sequentially across
/// turns instead of racing in parallel.
///
/// The dependency is only flagged when the batch contains *both* a context
/// producer (geolocation / datetime) and a consumer that builds a network
/// request (`fetch_url` / `extract_website`) — e.g. "look up my location"
/// emitted together with "search the web near me". In that case the consumers
/// are deferred and everything else (producers and any independent reads like
/// `read_file`) runs now. When only one side is present there is no
/// dependency and the whole batch stays [`BatchPlan::Independent`].
///
/// The bias is intentionally toward serializing: a false positive costs one
/// extra turn, while a false negative feeds the consumer stale or guessed
/// context.
#[must_use]
pub fn split_context_dependency(calls: Vec<ToolCall>) -> BatchPlan {
    let has_producer = calls.iter().any(is_context_producer);
    let has_consumer = calls.iter().any(is_context_consumer);
    if !(has_producer && has_consumer) {
        return BatchPlan::Independent(calls);
    }
    let (deferred, run_now): (Vec<_>, Vec<_>) = calls.into_iter().partition(is_context_consumer);
    BatchPlan::Dependency { run_now, deferred }
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

/// Names of the web-facing tools driven by the desktop's globe icon.
/// Co-located here so the security-denial branch can issue a targeted
/// "web tools are off" message instead of the generic "Security policy
/// denied" string.
const WEB_TOOLS: &[&str] = &[
    "search_web_fc",
    "search_web_ddg",
    "fetch_url",
    "extract_website",
];

/// Render the tool-result body produced when the security gate refuses
/// a call. Web-tool denials get hand-crafted messages so the model can
/// relay something actionable: when one search backend is off but its
/// counterpart is still enabled we steer the model to the sibling tool
/// instead of telling it to give up.
fn denial_message(tool_name: &str, reason: &str) -> String {
    if WEB_TOOLS.contains(&tool_name) && reason.contains("disabled") {
        let pol = crate::security::policy();
        let is_disabled = |name: &str| pol.disabled_tools.iter().any(|t| t == name);
        if tool_name == "search_web_fc" && !is_disabled("search_web_ddg") {
            return "`search_web_fc` is currently disabled by security policy. \
                Call `search_web_ddg` with the same query right now — it is the configured fallback and is still enabled. \
                Do not retry `search_web_fc` in this turn."
                .to_string();
        }
        if tool_name == "search_web_ddg" && !is_disabled("search_web_fc") {
            return "`search_web_ddg` is currently disabled by security policy. \
                Call `search_web_fc` with the same query instead — it is still enabled."
                .to_string();
        }
        return format!(
            "Web tools (`search_web_fc`, `search_web_ddg`, `fetch_url`, `extract_website`) are currently disabled, so `{tool_name}` cannot run. \
             Tell the user that you cannot fetch information from the web right now. \
             They can re-enable web tools by clicking the globe icon next to the Send button in the desktop app, \
             or by removing the entry from `AICTL_SECURITY_DISABLED_TOOLS` in `~/.aictl/config`. \
             Do not retry this tool until the user has done so."
        );
    }
    format!("Security policy denied: {reason}")
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
        return ToolOutput::text(denial_message(&tool_call.name, &reason));
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
        "search_web_fc" => web::tool_search_web_fc(input).await,
        "search_web_ddg" => web::tool_search_web_ddg(input).await,
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
        "test" => test::tool_test(input).await,
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
        "save_memory" => memory::tool_save_memory(input),
        "view_map" => view_map::tool_view_map(input).await,
        "draw_chart" => draw_chart::tool_draw_chart(input),
        other if other.starts_with("mcp__") => crate::mcp::call_tool(other, input).await,
        other => {
            if let Some(plugin) = crate::plugins::find(other) {
                crate::plugins::execute_plugin(&plugin, input).await
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

    // --- Multi-tool parsing (parse_tool_calls) ---

    #[test]
    fn parse_calls_empty_response_returns_empty_vec() {
        let resp = "Here is the answer to your question.";
        assert!(parse_tool_calls(resp).is_empty());
    }

    #[test]
    fn parse_calls_single_call_returns_one_entry() {
        let resp = r#"<tool name="read_file">src/main.rs</tool>"#;
        let calls = parse_tool_calls(resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].input, "src/main.rs");

        // Must match the single-call shape exactly.
        let single = parse_tool_call(resp).unwrap();
        assert_eq!(single.name, calls[0].name);
        assert_eq!(single.input, calls[0].input);
    }

    #[test]
    fn parse_calls_three_reads_in_source_order() {
        let resp = "Let me check three files.\n\
                    <tool name=\"read_file\">a.rs</tool>\n\
                    <tool name=\"read_file\">b.rs</tool>\n\
                    <tool name=\"read_file\">c.rs</tool>";
        let calls = parse_tool_calls(resp);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].input, "a.rs");
        assert_eq!(calls[1].input, "b.rs");
        assert_eq!(calls[2].input, "c.rs");
    }

    #[test]
    fn parse_calls_mixed_tool_names() {
        let resp = "<tool name=\"read_file\">a.rs</tool>\
                    <tool name=\"list_directory\">.</tool>\
                    <tool name=\"git\">status</tool>";
        let calls = parse_tool_calls(resp);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[1].name, "list_directory");
        assert_eq!(calls[2].name, "git");
        assert_eq!(calls[2].input, "status");
    }

    #[test]
    fn parse_calls_malformed_at_end_stops_scan() {
        // The malformed trailing block (missing close) terminates the scan;
        // earlier well-formed blocks are still collected.
        let resp = "<tool name=\"read_file\">a.rs</tool>\n\
                    <tool name=\"read_file\">broken";
        let calls = parse_tool_calls(resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].input, "a.rs");
    }

    // --- Side-effect classifier (is_parallelizable) ---

    fn call(name: &str, input: &str) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            input: input.to_string(),
        }
    }

    #[test]
    fn parallelizable_read_only_tools_return_true() {
        for name in &[
            "read_file",
            "list_directory",
            "search_files",
            "find_files",
            "lint_file",
            "check_port",
            "system_info",
            "fetch_url",
            "extract_website",
            "read_document",
            "json_query",
            "csv_query",
            "calculate",
            "fetch_datetime",
            "fetch_geolocation",
            "diff_files",
            "checksum",
            "list_processes",
        ] {
            assert!(
                is_parallelizable(&call(name, "")),
                "{name} should be parallelizable"
            );
        }
    }

    #[test]
    fn parallelizable_side_effect_tools_return_false() {
        for name in SIDE_EFFECT_TOOLS {
            assert!(
                !is_parallelizable(&call(name, "any body")),
                "{name} must not be parallelizable"
            );
        }
    }

    #[test]
    fn parallelizable_git_split_by_first_token() {
        for ro in &["status", "log", "blame", "diff", "log --oneline -n 5"] {
            assert!(
                is_parallelizable(&call("git", ro)),
                "git {ro} should be parallelizable"
            );
        }
        for se in &["commit -m \"x\"", "commit"] {
            assert!(
                !is_parallelizable(&call("git", se)),
                "git {se} must not be parallelizable"
            );
        }
    }

    #[test]
    fn parallelizable_clipboard_split_by_first_token() {
        assert!(is_parallelizable(&call("clipboard", "read")));
        assert!(is_parallelizable(&call("clipboard", "")));
        assert!(!is_parallelizable(&call("clipboard", "write\nhello")));
    }

    #[test]
    fn parallelizable_mcp_tools_return_false() {
        assert!(!is_parallelizable(&call("mcp__foo__bar", "{}")));
    }

    // --- Data-dependency split (split_context_dependency) ---

    fn names(calls: &[ToolCall]) -> Vec<&str> {
        calls.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn dependency_split_defers_consumer_after_producer() {
        let calls = vec![
            call("fetch_geolocation", ""),
            call("fetch_url", "https://example.com/weather?near=me"),
        ];
        match split_context_dependency(calls) {
            BatchPlan::Dependency { run_now, deferred } => {
                assert_eq!(names(&run_now), vec!["fetch_geolocation"]);
                assert_eq!(names(&deferred), vec!["fetch_url"]);
            }
            BatchPlan::Independent(_) => panic!("expected a dependency split"),
        }
    }

    #[test]
    fn dependency_split_keeps_independent_reads_with_producer() {
        // read_file does not consume location/time — it should run now, not
        // get deferred; only the consumer waits.
        let calls = vec![
            call("fetch_datetime", ""),
            call("read_file", "a.rs"),
            call("extract_website", "https://example.com"),
        ];
        match split_context_dependency(calls) {
            BatchPlan::Dependency { run_now, deferred } => {
                assert_eq!(names(&run_now), vec!["fetch_datetime", "read_file"]);
                assert_eq!(names(&deferred), vec!["extract_website"]);
            }
            BatchPlan::Independent(_) => panic!("expected a dependency split"),
        }
    }

    #[test]
    fn dependency_split_noop_without_producer() {
        // Consumer present but no producer — nothing to wait for.
        let calls = vec![
            call("fetch_url", "https://example.com"),
            call("read_file", "a.rs"),
        ];
        match split_context_dependency(calls) {
            BatchPlan::Independent(c) => assert_eq!(names(&c), vec!["fetch_url", "read_file"]),
            BatchPlan::Dependency { .. } => panic!("no producer — should stay independent"),
        }
    }

    #[test]
    fn dependency_split_noop_without_consumer() {
        // Two producers are independent of each other — run them in parallel.
        let calls = vec![call("fetch_geolocation", ""), call("fetch_datetime", "")];
        match split_context_dependency(calls) {
            BatchPlan::Independent(c) => {
                assert_eq!(names(&c), vec!["fetch_geolocation", "fetch_datetime"]);
            }
            BatchPlan::Dependency { .. } => panic!("no consumer — should stay independent"),
        }
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
