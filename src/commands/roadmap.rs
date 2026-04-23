//! `/roadmap` command — fetch `ROADMAP.md` from the project repo and render
//! it in the terminal via termimad.
//!
//! The fetched body is cached in [`CACHE`] for the life of the REPL session
//! so repeated invocations within a single run don't hit the network. An
//! optional section argument (`/roadmap desktop`) filters the output to the
//! matching heading block — useful when the full roadmap is long.

use crossterm::style::{Color, Stylize};
use tokio::sync::OnceCell;

use crate::agents::remote::{BRANCH, OWNER, REPO};
use crate::config::http_client;

/// Session-scoped cache of the fetched roadmap body. Populated on the first
/// successful call and reused thereafter. Failures are not cached — if the
/// user is offline we still want a retry to work once they reconnect.
static CACHE: OnceCell<String> = OnceCell::const_new();

const MAX_WIDTH: usize = 76;
const PAD: &str = "  ";
const FALLBACK_WIDTH: u16 = 80;

fn content_width() -> usize {
    let term = crossterm::terminal::size()
        .map(|(w, _)| w)
        .unwrap_or(FALLBACK_WIDTH) as usize;
    term.saturating_sub(4).min(MAX_WIDTH)
}

async fn fetch() -> Result<String, String> {
    let url = format!("https://raw.githubusercontent.com/{OWNER}/{REPO}/{BRANCH}/ROADMAP.md");
    let resp = http_client()
        .get(&url)
        .header("User-Agent", format!("aictl/{}", crate::VERSION))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub returned status {}", resp.status()));
    }
    resp.text()
        .await
        .map_err(|e| format!("failed to read response: {e}"))
}

/// Fetch (or reuse the cached body of) `ROADMAP.md` and print it.
///
/// `section`, when `Some`, is a case-insensitive substring matched against
/// heading titles; only the first matching section (up to the next heading at
/// the same or shallower depth) is rendered. When `None`, the whole document
/// is printed.
pub async fn run_roadmap(section: Option<&str>, show_error: &dyn Fn(&str)) {
    println!();
    println!("  {} fetching roadmap...", "↓".with(Color::Cyan));

    let body = match CACHE.get_or_try_init(fetch).await {
        Ok(b) => b,
        Err(e) => {
            show_error(&format!("Could not fetch ROADMAP.md: {e}"));
            return;
        }
    };

    let to_render = match section {
        None => body.as_str(),
        Some(q) => {
            if let Some(slice) = extract_section(body, q) {
                slice
            } else {
                show_error(&format!("No heading matches '{q}' in the roadmap."));
                return;
            }
        }
    };

    let skin = termimad::MadSkin::default();
    let rendered = format!(
        "{}",
        termimad::FmtText::from_text(&skin, to_render.into(), Some(content_width()))
    );
    println!();
    for line in rendered.lines() {
        println!("{PAD}{line}");
    }
    println!();
}

/// Find the first ATX heading whose title contains `query` (case-insensitive)
/// and return the slice of `body` from that heading up to — but not including
/// — the next heading at the same or shallower depth. Returns `None` if no
/// heading matches.
fn extract_section<'a>(body: &'a str, query: &str) -> Option<&'a str> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }

    let mut start: Option<(usize, usize)> = None; // (byte offset, depth)
    let mut end = body.len();
    let mut offset = 0usize;

    for line in body.split_inclusive('\n') {
        if let Some(depth) = heading_depth(line) {
            if let Some((_, start_depth)) = start {
                if depth <= start_depth {
                    end = offset;
                    break;
                }
            } else {
                let title = line.trim_start_matches('#').trim();
                if title.to_ascii_lowercase().contains(&q) {
                    start = Some((offset, depth));
                }
            }
        }
        offset += line.len();
    }

    start.map(|(s, _)| body[s..end].trim_end_matches('\n'))
}

/// Return `Some(n)` if `line` is a level-`n` ATX heading (1..=6). Anything
/// else (code fences, blockquotes, non-heading content) returns `None`.
fn heading_depth(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r']) {
        Some(hashes)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Roadmap

## General

### REPL

- item a
- item b

## Desktop

Body about desktop.

### Phased rollout

Phased body.

## Mobile

Mobile body.
";

    #[test]
    fn extracts_top_level_section() {
        let out = extract_section(SAMPLE, "desktop").unwrap();
        assert!(out.starts_with("## Desktop"));
        assert!(out.contains("Body about desktop."));
        assert!(out.contains("### Phased rollout"));
        assert!(!out.contains("## Mobile"));
    }

    #[test]
    fn extracts_subsection_without_leaking_into_sibling_top_level() {
        let out = extract_section(SAMPLE, "phased").unwrap();
        assert!(out.starts_with("### Phased rollout"));
        assert!(out.contains("Phased body."));
        assert!(!out.contains("## Mobile"));
    }

    #[test]
    fn case_insensitive_match() {
        let out = extract_section(SAMPLE, "REPL").unwrap();
        assert!(out.starts_with("### REPL"));
        let out2 = extract_section(SAMPLE, "repl").unwrap();
        assert_eq!(out, out2);
    }

    #[test]
    fn unknown_section_returns_none() {
        assert!(extract_section(SAMPLE, "nonexistent").is_none());
        assert!(extract_section(SAMPLE, "   ").is_none());
    }

    #[test]
    fn heading_depth_variants() {
        assert_eq!(heading_depth("# H1\n"), Some(1));
        assert_eq!(heading_depth("### H3\n"), Some(3));
        assert_eq!(heading_depth("###### H6\n"), Some(6));
        assert_eq!(heading_depth("####### too deep\n"), None);
        assert_eq!(heading_depth("not a heading\n"), None);
        // `#foo` (no space) is not a valid ATX heading.
        assert_eq!(heading_depth("#foo\n"), None);
        // Trailing blank heading (`#\n`) is still a heading per CommonMark.
        assert_eq!(heading_depth("#\n"), Some(1));
    }
}
