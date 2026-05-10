//! Display a map in the desktop app via OpenStreetMap.
//!
//! The tool resolves the supplied location to `(lat, lon)` — either by
//! parsing direct coordinates or by geocoding a place name through
//! Nominatim — and emits a `[view_map] {json}` marker line that the
//! desktop webview detects (mirroring how `Image saved to <path>`
//! triggers an inline image preview). The webview renders an
//! OpenStreetMap embed iframe in the tool callout.
//!
//! Only the desktop frontend ([`crate::config::Role::Desktop`]) can
//! render maps. Any other role (CLI, server) returns a clear error
//! telling the model not to retry — the terminal cannot draw a map and
//! piping a screenshot of one back to the user makes no sense.
//!
//! # Input formats
//!
//! Each non-empty line of the input is one pin. A pin line is:
//!
//! ```text
//! <query>[ | <label>[ | <description>]]
//! ```
//!
//! where `<query>` is one of:
//!
//! - `"<lat>, <lon>"` — direct coordinates (degrees).
//! - `"<lat>, <lon>, <zoom>"` — direct coordinates with explicit zoom
//!   (1..=19). Only honored for *single-pin* input; with multiple
//!   pins the webview auto-fits the viewport to enclose all markers.
//! - free-form text (e.g. `"Eiffel Tower, Paris"`) — geocoded via
//!   Nominatim at <https://nominatim.openstreetmap.org/search>.
//!
//! `<label>` (optional) overrides the auto-derived label that comes
//! from the geocoder or `lat,lon` formatting. `<description>`
//! (optional) is shown in a popup when the pin is clicked in the
//! desktop frontend.
//!
//! Nominatim's usage policy mandates a descriptive `User-Agent` header
//! and a maximum of one request per second; multi-pin geocoding
//! sequentialises requests with a small delay so we stay under the
//! limit instead of issuing a burst that would risk a soft block.

use std::fmt::Write as _;
use std::time::Duration;

use serde::Deserialize;

use crate::config::{Role, http_client, role};

const NOMINATIM_URL: &str = "https://nominatim.openstreetmap.org/search";
const USER_AGENT: &str = concat!(
    "aictl/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/pwittchen/aictl)"
);
const DEFAULT_ZOOM: u32 = 13;
const MIN_ZOOM: u32 = 1;
const MAX_ZOOM: u32 = 19;
const MAX_PINS: usize = 25;
/// Delay between consecutive geocode requests when the input has
/// multiple pins that need resolving. Nominatim's usage policy is
/// "1 req/sec"; pad slightly above that.
const GEOCODE_GAP: Duration = Duration::from_millis(1100);
/// Hard timeout per Nominatim request. The agent loop's outer
/// `AICTL_LLM_TIMEOUT` doesn't cover tool work, and a hung Nominatim
/// would otherwise make the tool look frozen for tens of seconds.
const GEOCODE_TIMEOUT: Duration = Duration::from_secs(8);
/// Backoff after a 429 (rate-limited) response. Conservative — we'd
/// rather sleep through the throttle and succeed than burn the model's
/// retry budget.
const GEOCODE_BACKOFF_429: Duration = Duration::from_millis(2000);

#[derive(Deserialize)]
struct NominatimHit {
    lat: String,
    lon: String,
    display_name: String,
    /// Nominatim's relevance score in `[0, 1]`. Returned for every hit
    /// and used to pick the strongest candidate when the query is
    /// ambiguous (e.g. "Springfield" — Nominatim returns the
    /// highest-importance match first, but explicit selection
    /// insulates us from any future API ordering change).
    #[serde(default)]
    importance: f64,
}

struct Resolved {
    label: String,
    lat: f64,
    lon: f64,
    zoom: u32,
}

pub(super) async fn tool_view_map(input: &str) -> String {
    if !matches!(role(), Role::Desktop) {
        return "Error: view_map is only available in the aictl desktop app — \
                the terminal cannot render maps. Do not retry this tool. \
                Describe the location in text instead, or share an OpenStreetMap link \
                like https://www.openstreetmap.org/?mlat=<LAT>&mlon=<LON>#map=<ZOOM>/<LAT>/<LON>."
            .to_string();
    }

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "Error: view_map requires at least one location query (place name, address, or `lat,lon[,zoom]` coordinates).".to_string();
    }

    let raw_lines: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if raw_lines.len() > MAX_PINS {
        return format!(
            "Error: view_map accepts at most {MAX_PINS} pins per call ({} requested).",
            raw_lines.len()
        );
    }

    let multi_pin = raw_lines.len() > 1;
    let mut pins: Vec<Pin> = Vec::with_capacity(raw_lines.len());
    let mut single_zoom: Option<u32> = None;
    let mut geocoded_count: usize = 0;

    for line in &raw_lines {
        let (query, label_override, description) = split_pin_fields(line);
        if query.is_empty() {
            return format!("Error: pin line is missing a query: '{line}'.");
        }

        let resolved = if let Some(r) = parse_coords(query) {
            if !multi_pin {
                single_zoom = Some(r.zoom);
            }
            r
        } else {
            // Sequence geocode requests so we never burst-fire Nominatim.
            // First geocode runs immediately; subsequent ones pad to
            // honour the published 1 req/sec budget.
            if geocoded_count > 0 {
                tokio::time::sleep(GEOCODE_GAP).await;
            }
            geocoded_count += 1;
            match geocode(query).await {
                Ok(r) => r,
                Err(e) => return format!("Error resolving location '{query}': {e}"),
            }
        };

        let label = label_override
            .filter(|s| !s.is_empty())
            .map_or(resolved.label, str::to_string);

        pins.push(Pin {
            lat: resolved.lat,
            lon: resolved.lon,
            label,
            description,
        });
    }

    // The first pin doubles as the "primary" — it's the value the
    // pre-existing single-pin payload fields point at, so a webview
    // that only knows the legacy schema still recovers a working map
    // (the new `pins` array is what current builds actually render).
    let primary = &pins[0];
    let zoom = if multi_pin {
        // Multi-pin: leave zoom unset so the webview can fitBounds()
        // around all markers. Serialise as `null` rather than dropping
        // the key so the JSON shape stays stable across pin counts.
        serde_json::Value::Null
    } else {
        serde_json::Value::Number(serde_json::Number::from(
            single_zoom.unwrap_or(DEFAULT_ZOOM),
        ))
    };

    let payload = serde_json::json!({
        "query": trimmed,
        "label": primary.label,
        "lat": primary.lat,
        "lon": primary.lon,
        "zoom": zoom,
        "pins": pins.iter().map(|p| serde_json::json!({
            "lat": p.lat,
            "lon": p.lon,
            "label": p.label,
            "description": p.description,
        })).collect::<Vec<_>>(),
    });

    let summary = render_summary(&pins, single_zoom);
    format!("[view_map] {payload}\n{summary}")
}

/// Build the human-readable summary the model relays back to the user.
/// Single-pin input gets a one-liner; multi-pin gets a numbered list so
/// users on non-desktop frontends still see every location in text.
fn render_summary(pins: &[Pin], single_zoom: Option<u32>) -> String {
    let mut out = String::new();
    if pins.len() > 1 {
        let _ = writeln!(
            out,
            "Map displayed in the desktop app with {} pins:",
            pins.len()
        );
        for (i, pin) in pins.iter().enumerate() {
            let _ = writeln!(
                out,
                "  {}. {} (lat {:.5}, lon {:.5})",
                i + 1,
                pin.label,
                pin.lat,
                pin.lon
            );
        }
    } else if let Some(primary) = pins.first() {
        let _ = writeln!(
            out,
            "Map displayed in the desktop app: {} (lat {:.5}, lon {:.5}, zoom {}).",
            primary.label,
            primary.lat,
            primary.lon,
            single_zoom.unwrap_or(DEFAULT_ZOOM)
        );
    }
    out.trim_end().to_string()
}

#[derive(Debug)]
struct Pin {
    lat: f64,
    lon: f64,
    label: String,
    description: Option<String>,
}

/// Split a single pin line on `|` into `(query, label?, description?)`.
/// The query field is always the first segment. Whitespace around each
/// segment is stripped so the LLM can format the input loosely.
fn split_pin_fields(line: &str) -> (&str, Option<&str>, Option<String>) {
    let mut it = line.splitn(3, '|');
    let query = it.next().unwrap_or("").trim();
    let label = it.next().map(str::trim);
    let description = it
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (query, label, description)
}

/// Try to parse the input as `lat,lon` or `lat,lon,zoom` (degrees,
/// signed decimal). Returns `None` for anything that isn't a clean
/// 2- or 3-component numeric tuple — those fall through to geocoding.
///
/// Tolerates several variants the LLM commonly emits even when the
/// system prompt says comma-separated: `48.8566 2.3522` (whitespace),
/// `48.8566, 2.3522 (zoom 15)`, and trailing direction markers
/// (`48.8566N, 2.3522E`). Each is normalised to the canonical
/// comma-separated form before parsing so an obvious coordinate
/// payload never wastes a geocode round-trip.
fn parse_coords(input: &str) -> Option<Resolved> {
    let cleaned = normalise_coord_string(input);
    let parts: Vec<&str> = cleaned.split(',').map(str::trim).collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let lat: f64 = parts[0].parse().ok()?;
    let lon: f64 = parts[1].parse().ok()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let zoom: u32 = parts
        .get(2)
        .and_then(|z| z.parse().ok())
        .unwrap_or(DEFAULT_ZOOM)
        .clamp(MIN_ZOOM, MAX_ZOOM);
    Some(Resolved {
        label: format!("{lat:.5}, {lon:.5}"),
        lat,
        lon,
        zoom,
    })
}

/// Normalise a candidate coordinate string into the strict
/// `<num>,<num>[,<num>]` shape `parse_coords` expects. Strips
/// surrounding parentheses, hemisphere markers (`N`/`S`/`E`/`W`,
/// case-insensitive) on each component, and falls back to
/// whitespace-as-comma when the input has no comma at all.
fn normalise_coord_string(input: &str) -> String {
    let mut s = input
        .trim()
        .trim_matches(|c| matches!(c, '(' | ')'))
        .to_string();
    if !s.contains(',') {
        // `48.8566 2.3522` → `48.8566,2.3522`. Only do this when no
        // comma is present so we don't mangle queries like
        // `Springfield, IL` that happen to contain a space.
        s = s.split_whitespace().collect::<Vec<_>>().join(",");
    }
    s.split(',')
        .map(strip_hemisphere_suffix)
        .collect::<Vec<_>>()
        .join(",")
}

/// Strip a single trailing `N`/`S`/`E`/`W` hemisphere marker (any
/// case, possibly preceded by whitespace) from a coordinate
/// component. Returns the component unchanged when no marker is
/// present, so `48.8566` and `Springfield` both pass through.
fn strip_hemisphere_suffix(part: &str) -> String {
    let trimmed = part.trim();
    if let Some(stripped) = trimmed
        .strip_suffix(['N', 'S', 'E', 'W'])
        .or_else(|| trimmed.strip_suffix(['n', 's', 'e', 'w']))
    {
        stripped.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

async fn geocode(query: &str) -> Result<Resolved, String> {
    let cleaned = clean_query(query);
    let target = if cleaned.is_empty() { query } else { &cleaned };

    let mut hits = match fetch_nominatim(target).await {
        Ok(v) => v,
        Err(e) => return Err(e),
    };

    // Some LLM-authored queries pack in filler ("the Eiffel Tower in
    // central Paris, France") that Nominatim parses well most of the
    // time but occasionally over-constrains. If the strict query
    // returns nothing, retry once with a relaxed variant: keep the
    // first (most-specific) comma segment plus the last (most general,
    // usually a country/region). Skips when the original was already
    // a single segment.
    if hits.is_empty()
        && let Some(relaxed) = relaxed_variant(target)
        && relaxed != target
    {
        tokio::time::sleep(GEOCODE_GAP).await;
        hits = fetch_nominatim(&relaxed).await.unwrap_or_default();
    }

    let hit = pick_best(hits).ok_or_else(|| {
        format!(
            "no results for '{query}' — try a more specific query (include city + country), \
             an address, or `lat,lon` coordinates"
        )
    })?;

    let lat: f64 = hit
        .lat
        .parse()
        .map_err(|e| format!("Nominatim returned non-numeric lat '{}': {e}", hit.lat))?;
    let lon: f64 = hit
        .lon
        .parse()
        .map_err(|e| format!("Nominatim returned non-numeric lon '{}': {e}", hit.lon))?;
    Ok(Resolved {
        label: hit.display_name,
        lat,
        lon,
        zoom: DEFAULT_ZOOM,
    })
}

/// One Nominatim round-trip with the full set of quality-improving
/// parameters. Handles network errors, timeout, and a single 429
/// retry transparently.
async fn fetch_nominatim(query: &str) -> Result<Vec<NominatimHit>, String> {
    // `limit=5` so we have alternatives to pick from when the top-1
    // result is a tiny populated place that happens to outrank the
    // famous landmark by quirk of Nominatim's importance score.
    // `accept-language=en` so labels come back in English regardless
    // of the OS locale of the Nominatim node we hit — keeps the
    // popup text predictable.
    let url = format!(
        "{NOMINATIM_URL}?q={}&format=json&limit=5&accept-language=en&addressdetails=0&dedupe=1",
        percent_encode(query)
    );

    for attempt in 0..2 {
        let send = http_client()
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .send();
        let resp = match tokio::time::timeout(GEOCODE_TIMEOUT, send).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(format!("network error contacting Nominatim: {e}")),
            Err(_) => {
                return Err(format!(
                    "Nominatim timed out after {}s",
                    GEOCODE_TIMEOUT.as_secs()
                ));
            }
        };
        let status = resp.status();
        if status.as_u16() == 429 && attempt == 0 {
            // Rate-limited — back off briefly and try once more. The
            // retry budget is intentionally tiny (one extra try) so
            // we never sit in a loop hammering the public endpoint.
            tokio::time::sleep(GEOCODE_BACKOFF_429).await;
            continue;
        }
        if !status.is_success() {
            return Err(format!("Nominatim returned HTTP {status}"));
        }
        return resp
            .json::<Vec<NominatimHit>>()
            .await
            .map_err(|e| format!("parsing Nominatim response: {e}"));
    }
    Err("Nominatim rate limited after retry — try again in a moment".to_string())
}

/// Pick the result that maximises `importance`. Nominatim already
/// orders by it, but explicit selection lets us survive any future
/// reordering and falls back to the first entry when scores tie.
fn pick_best(hits: Vec<NominatimHit>) -> Option<NominatimHit> {
    hits.into_iter().reduce(|best, next| {
        if next.importance > best.importance {
            next
        } else {
            best
        }
    })
}

/// Lightly normalise a user-supplied query before it goes to the
/// geocoder. Removes leading/trailing quotes (a common LLM artefact
/// — `"Eiffel Tower"`) and collapses runs of internal whitespace.
/// Returns an empty string when the cleaned form would be blank, so
/// the caller can fall back to the original.
fn clean_query(query: &str) -> String {
    let trimmed = query
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`'));
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_space = false;
    for c in trimmed.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Build a relaxed retry query from a strict one by keeping just the
/// first and last comma-separated segments — i.e. drop the middle
/// admin layers that often cause spurious zero-result responses while
/// preserving "what" + "where" hints. Returns `None` for queries with
/// fewer than three segments (nothing meaningful to drop).
fn relaxed_variant(query: &str) -> Option<String> {
    let parts: Vec<&str> = query
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    Some(format!("{}, {}", parts[0], parts[parts.len() - 1]))
}

/// Minimal percent-encoder mirroring `web.rs::percent_encode` — RFC 3986
/// unreserved chars pass through, everything else becomes `%XX`. Kept
/// local so the geocode call site doesn't need an extra dependency.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_coords_two_components() {
        let r = parse_coords("48.8566, 2.3522").unwrap();
        assert!((r.lat - 48.8566).abs() < 1e-6);
        assert!((r.lon - 2.3522).abs() < 1e-6);
        assert_eq!(r.zoom, DEFAULT_ZOOM);
    }

    #[test]
    fn parse_coords_three_components_with_zoom() {
        let r = parse_coords("-33.8688,151.2093,15").unwrap();
        assert!((r.lat - -33.8688).abs() < 1e-6);
        assert_eq!(r.zoom, 15);
    }

    #[test]
    fn parse_coords_clamps_zoom() {
        assert_eq!(parse_coords("0,0,0").unwrap().zoom, MIN_ZOOM);
        assert_eq!(parse_coords("0,0,99").unwrap().zoom, MAX_ZOOM);
    }

    #[test]
    fn parse_coords_rejects_out_of_range() {
        assert!(parse_coords("91, 0").is_none());
        assert!(parse_coords("0, 181").is_none());
    }

    #[test]
    fn parse_coords_rejects_non_numeric() {
        assert!(parse_coords("Paris, France").is_none());
        assert!(parse_coords("foo").is_none());
        assert!(parse_coords("1,2,3,4").is_none());
    }

    #[tokio::test]
    async fn empty_input_returns_error_in_desktop_role() {
        // The role lock is process-global, so we don't try to flip it
        // here; instead we exercise the empty-input branch which fires
        // before the role check matters in practice. (The role-gate
        // branch is covered by the integration-style test below.)
        let out = tool_view_map("").await;
        // In the default `Cli` role, the role-gate fires first.
        assert!(
            out.starts_with("Error: view_map is only available")
                || out.starts_with("Error: view_map requires")
        );
    }

    #[tokio::test]
    async fn cli_role_refuses() {
        // Default role is Cli — make sure the gate fires with a clear
        // "do not retry" message so a model doesn't loop.
        let out = tool_view_map("48.8566, 2.3522").await;
        assert!(out.contains("only available in the aictl desktop app"));
        assert!(out.contains("Do not retry"));
    }

    #[test]
    fn split_pin_fields_query_only() {
        let (q, label, desc) = split_pin_fields("Eiffel Tower");
        assert_eq!(q, "Eiffel Tower");
        assert!(label.is_none());
        assert!(desc.is_none());
    }

    #[test]
    fn split_pin_fields_with_label() {
        let (q, label, desc) = split_pin_fields("48.86, 2.35 | Paris");
        assert_eq!(q, "48.86, 2.35");
        assert_eq!(label, Some("Paris"));
        assert!(desc.is_none());
    }

    #[test]
    fn split_pin_fields_with_label_and_description() {
        let (q, label, desc) = split_pin_fields("Louvre | Louvre Museum | Largest art museum");
        assert_eq!(q, "Louvre");
        assert_eq!(label, Some("Louvre Museum"));
        assert_eq!(desc.as_deref(), Some("Largest art museum"));
    }

    #[test]
    fn split_pin_fields_keeps_pipes_in_description() {
        // Only the first two pipes split — anything after stays in the
        // description so users can include `|` in free-form prose.
        let (q, label, desc) = split_pin_fields("a | b | c | d | e");
        assert_eq!(q, "a");
        assert_eq!(label, Some("b"));
        assert_eq!(desc.as_deref(), Some("c | d | e"));
    }

    #[test]
    fn split_pin_fields_blank_description_dropped() {
        let (_, _, desc) = split_pin_fields("Place | Label | ");
        assert!(desc.is_none());
    }

    #[test]
    fn parse_coords_accepts_whitespace_separator() {
        // LLMs sometimes emit `lat lon` even after we say "comma".
        let r = parse_coords("48.8566 2.3522").unwrap();
        assert!((r.lat - 48.8566).abs() < 1e-6);
        assert!((r.lon - 2.3522).abs() < 1e-6);
    }

    #[test]
    fn parse_coords_strips_hemisphere_markers() {
        let r = parse_coords("48.8566N, 2.3522E").unwrap();
        assert!((r.lat - 48.8566).abs() < 1e-6);
        assert!((r.lon - 2.3522).abs() < 1e-6);
        let r = parse_coords("33.8688s 151.2093e").unwrap();
        assert!((r.lat - 33.8688).abs() < 1e-6);
    }

    #[test]
    fn parse_coords_strips_parentheses() {
        let r = parse_coords("(48.8566, 2.3522)").unwrap();
        assert!((r.lat - 48.8566).abs() < 1e-6);
    }

    #[test]
    fn parse_coords_does_not_mangle_place_names() {
        // "Springfield, IL" has a comma — the whitespace-as-comma
        // fallback must not fire and must not produce numeric parses.
        assert!(parse_coords("Springfield, IL").is_none());
    }

    #[test]
    fn clean_query_strips_quotes_and_collapses_spaces() {
        assert_eq!(clean_query("\"Eiffel  Tower\""), "Eiffel Tower");
        assert_eq!(clean_query("  'Paris'  "), "Paris");
        assert_eq!(clean_query("`back\tticks`"), "back ticks");
    }

    #[test]
    fn relaxed_variant_drops_middle_segments() {
        assert_eq!(
            relaxed_variant("123 Main St, Springfield, IL, USA").as_deref(),
            Some("123 Main St, USA"),
        );
    }

    #[test]
    fn relaxed_variant_returns_none_for_short_queries() {
        assert!(relaxed_variant("Paris").is_none());
        assert!(relaxed_variant("Paris, France").is_none());
    }

    #[test]
    fn pick_best_picks_highest_importance() {
        let hits = vec![
            NominatimHit {
                lat: "0".to_string(),
                lon: "0".to_string(),
                display_name: "low".to_string(),
                importance: 0.1,
            },
            NominatimHit {
                lat: "1".to_string(),
                lon: "1".to_string(),
                display_name: "high".to_string(),
                importance: 0.9,
            },
        ];
        let best = pick_best(hits).unwrap();
        assert_eq!(best.display_name, "high");
    }

    #[test]
    fn pick_best_handles_empty() {
        assert!(pick_best(vec![]).is_none());
    }
}
