//! Display a chart in the desktop app via Chart.js.
//!
//! The tool accepts a JSON description of a chart (type, optional title
//! and axis labels, category labels, one or more named data series) and
//! emits a `[draw_chart] {json}` marker line that the desktop webview
//! detects (mirroring how `view_map` is intercepted). The webview
//! renders a Chart.js canvas in the tool callout and re-themes the
//! chart on the fly when the app theme (or OS preference, under
//! `system` mode) changes.
//!
//! Only the desktop frontend ([`crate::config::Role::Desktop`]) can
//! render charts. Any other role (CLI, server) returns a clear error
//! telling the model not to retry — the terminal cannot draw a chart
//! and screenshotting one back would be senseless.
//!
//! # Input format
//!
//! A single JSON object:
//!
//! ```text
//! {
//!   "type": "line" | "bar" | "pie" | "doughnut" | "scatter",
//!   "title": "optional title",
//!   "x_label": "optional x-axis label",        // line / bar / scatter
//!   "y_label": "optional y-axis label",        // line / bar / scatter
//!   "labels": ["A", "B", "C"],                 // category labels (not used by scatter)
//!   "series": [
//!     { "name": "Series 1", "data": [1, 2, 3] },
//!     // scatter takes [x,y] pairs:
//!     // { "name": "Series 1", "data": [[1, 2], [3, 4]] }
//!   ]
//! }
//! ```
//!
//! For `pie` / `doughnut` only a single series is allowed (the slice
//! values). For category charts (`line` / `bar` / `pie` / `doughnut`)
//! each series's `data` length must equal the number of `labels`.

use serde_json::{Map, Value};

use crate::config::{Role, role};

const MAX_SERIES: usize = 8;
const MAX_POINTS_PER_SERIES: usize = 500;
const MAX_TITLE_LEN: usize = 200;
const MAX_LABEL_LEN: usize = 80;
const MAX_LABELS: usize = 500;
const MAX_SERIES_NAME_LEN: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ChartKind {
    Line,
    Bar,
    Pie,
    Doughnut,
    Scatter,
}

impl ChartKind {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "line" => Some(Self::Line),
            "bar" => Some(Self::Bar),
            "pie" => Some(Self::Pie),
            "doughnut" | "donut" => Some(Self::Doughnut),
            "scatter" => Some(Self::Scatter),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Bar => "bar",
            Self::Pie => "pie",
            Self::Doughnut => "doughnut",
            Self::Scatter => "scatter",
        }
    }

    /// `true` when the chart uses a categorical x-axis (one value per
    /// label). `false` for scatter (x/y pairs).
    fn is_category(self) -> bool {
        matches!(self, Self::Line | Self::Bar | Self::Pie | Self::Doughnut)
    }

    fn allows_multiple_series(self) -> bool {
        !matches!(self, Self::Pie | Self::Doughnut)
    }

    fn allows_negative_values(self) -> bool {
        !matches!(self, Self::Pie | Self::Doughnut)
    }
}

pub(super) fn tool_draw_chart(input: &str) -> String {
    if !matches!(role(), Role::Desktop) {
        return "Error: draw_chart is only available in the aictl desktop app — \
                the terminal cannot render charts. Do not retry this tool. \
                Describe the data as a Markdown table instead."
            .to_string();
    }

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "Error: draw_chart requires a JSON object describing the chart. \
                Example: {\"type\":\"bar\",\"title\":\"Sales\",\"labels\":[\"Q1\",\"Q2\"],\"series\":[{\"name\":\"2024\",\"data\":[10,20]}]}"
            .to_string();
    }

    match build_payload(trimmed) {
        Ok((payload, summary)) => format!("[draw_chart] {payload}\n{summary}"),
        Err(msg) => msg,
    }
}

/// Parse + validate + normalise the JSON input. Split out so
/// `tool_draw_chart` stays a thin wrapper that handles the role gate
/// and the final marker formatting; all the shape-checking lives here
/// and is unit-testable without going through the role gate.
fn build_payload(trimmed: &str) -> Result<(Value, String), String> {
    let raw: Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("Error: draw_chart input is not valid JSON: {e}"))?;
    let Some(obj) = raw.as_object() else {
        return Err("Error: draw_chart input must be a JSON object.".to_string());
    };

    let kind = parse_kind(obj)?;
    let title = parse_optional_string(obj, "title", MAX_TITLE_LEN)?;
    let x_label = parse_optional_string(obj, "x_label", MAX_LABEL_LEN)?;
    let y_label = parse_optional_string(obj, "y_label", MAX_LABEL_LEN)?;
    let labels = parse_labels(obj)?;

    let Some(Value::Array(series_raw)) = obj.get("series") else {
        return Err(match obj.get("series") {
            Some(_) => "Error: `series` must be an array of objects.".to_string(),
            None => "Error: missing `series` field.".to_string(),
        });
    };

    if series_raw.is_empty() {
        return Err("Error: draw_chart requires at least one series.".to_string());
    }
    if series_raw.len() > MAX_SERIES {
        return Err(format!(
            "Error: too many series ({}) — limit is {MAX_SERIES}.",
            series_raw.len()
        ));
    }
    if !kind.allows_multiple_series() && series_raw.len() > 1 {
        return Err(format!(
            "Error: {} charts accept only one series (got {}). Use a bar chart to compare multiple datasets.",
            kind.as_str(),
            series_raw.len()
        ));
    }
    if kind.is_category() && labels.is_empty() {
        return Err(format!(
            "Error: {} chart requires a `labels` array (one entry per data point).",
            kind.as_str()
        ));
    }

    let normalised_series = normalise_series(series_raw, kind, labels.len())?;

    let payload = serde_json::json!({
        "type": kind.as_str(),
        "title": title,
        "x_label": x_label,
        "y_label": y_label,
        "labels": labels,
        "series": normalised_series,
    });
    let summary = render_summary(kind, title.as_deref(), series_raw.len(), labels.len());
    Ok((payload, summary))
}

fn parse_kind(obj: &Map<String, Value>) -> Result<ChartKind, String> {
    let Some(kind_str) = obj.get("type").and_then(Value::as_str) else {
        return Err(
            "Error: missing `type` field (must be one of line, bar, pie, doughnut, scatter)."
                .to_string(),
        );
    };
    ChartKind::parse(kind_str).ok_or_else(|| {
        format!(
            "Error: unknown chart type '{kind_str}'. Supported: line, bar, pie, doughnut, scatter."
        )
    })
}

fn parse_optional_string(
    obj: &Map<String, Value>,
    key: &str,
    max_len: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(s)) if s.len() <= max_len => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(format!("Error: {key} exceeds {max_len} characters.")),
        Some(_) => Err(format!("Error: {key} must be a string.")),
    }
}

fn parse_labels(obj: &Map<String, Value>) -> Result<Vec<String>, String> {
    let arr = match obj.get("labels") {
        Some(Value::Array(arr)) => arr,
        Some(Value::Null) | None => return Ok(Vec::new()),
        Some(_) => return Err("Error: `labels` must be an array of strings.".to_string()),
    };
    if arr.len() > MAX_LABELS {
        return Err(format!(
            "Error: too many labels ({}) — limit is {MAX_LABELS}.",
            arr.len()
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        match v.as_str() {
            Some(s) if s.len() <= MAX_LABEL_LEN => out.push(s.to_string()),
            Some(_) => {
                return Err(format!(
                    "Error: label #{i} exceeds {MAX_LABEL_LEN} characters."
                ));
            }
            None => return Err(format!("Error: label #{i} must be a string.")),
        }
    }
    Ok(out)
}

fn normalise_series(
    series_raw: &[Value],
    kind: ChartKind,
    label_count: usize,
) -> Result<Vec<Value>, String> {
    series_raw
        .iter()
        .enumerate()
        .map(|(i, item)| normalise_one_series(i, item, kind, label_count))
        .collect()
}

fn normalise_one_series(
    i: usize,
    item: &Value,
    kind: ChartKind,
    label_count: usize,
) -> Result<Value, String> {
    let Some(s_obj) = item.as_object() else {
        return Err(format!("Error: series #{i} must be an object."));
    };
    let Some(name) = s_obj.get("name").and_then(Value::as_str) else {
        return Err(format!("Error: series #{i} is missing `name`."));
    };
    if name.is_empty() {
        return Err(format!("Error: series #{i} has an empty `name`."));
    }
    if name.len() > MAX_SERIES_NAME_LEN {
        return Err(format!(
            "Error: series #{i} name exceeds {MAX_SERIES_NAME_LEN} characters."
        ));
    }
    let Some(data_value) = s_obj.get("data") else {
        return Err(format!("Error: series '{name}' is missing `data`."));
    };

    let data = if kind.is_category() {
        normalise_numeric_data(name, data_value, kind, label_count)?
    } else {
        normalise_scatter_data(name, data_value)?
    };

    Ok(serde_json::json!({ "name": name, "data": data }))
}

fn normalise_numeric_data(
    name: &str,
    value: &Value,
    kind: ChartKind,
    label_count: usize,
) -> Result<Value, String> {
    let nums = parse_numeric_series(value)
        .map_err(|e| format!("Error: series '{name}' has invalid data — {e}"))?;
    if nums.len() != label_count {
        return Err(format!(
            "Error: series '{name}' has {} data points but there are {label_count} labels — counts must match for {} charts.",
            nums.len(),
            kind.as_str()
        ));
    }
    if !kind.allows_negative_values() && nums.iter().any(|v| *v < 0.0) {
        return Err(format!(
            "Error: {} charts cannot display negative values (series '{name}' contains one).",
            kind.as_str()
        ));
    }
    Ok(Value::Array(
        nums.into_iter()
            .map(|n| serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number))
            .collect(),
    ))
}

fn normalise_scatter_data(name: &str, value: &Value) -> Result<Value, String> {
    let pairs =
        parse_scatter_series(value).map_err(|e| format!("Error: scatter series '{name}' — {e}"))?;
    Ok(Value::Array(
        pairs
            .into_iter()
            .map(|(x, y)| serde_json::json!({ "x": x, "y": y }))
            .collect(),
    ))
}

/// Parse a category-chart `data` array into `Vec<f64>`. Rejects
/// non-numeric entries and non-finite values (NaN/Inf) up-front so
/// Chart.js never sees a broken payload it would render as a gap.
fn parse_numeric_series(value: &Value) -> Result<Vec<f64>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "data must be an array of numbers".to_string())?;
    if arr.len() > MAX_POINTS_PER_SERIES {
        return Err(format!(
            "too many points ({}) — limit is {MAX_POINTS_PER_SERIES}",
            arr.len()
        ));
    }
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            let n = v
                .as_f64()
                .ok_or_else(|| format!("point #{i} is not numeric (got {})", short_json_kind(v)))?;
            if !n.is_finite() {
                return Err(format!("point #{i} is not a finite number"));
            }
            Ok(n)
        })
        .collect()
}

/// Parse a scatter `data` array of `[x, y]` pairs (or `{x, y}` objects).
/// Both shapes are accepted because LLMs flip-flop between them.
fn parse_scatter_series(value: &Value) -> Result<Vec<(f64, f64)>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "data must be an array of [x, y] pairs".to_string())?;
    if arr.len() > MAX_POINTS_PER_SERIES {
        return Err(format!(
            "too many points ({}) — limit is {MAX_POINTS_PER_SERIES}",
            arr.len()
        ));
    }
    arr.iter()
        .enumerate()
        .map(|(i, v)| extract_xy(v).map_err(|e| format!("point #{i}: {e}")))
        .collect()
}

fn extract_xy(v: &Value) -> Result<(f64, f64), String> {
    if let Some(arr) = v.as_array() {
        if arr.len() != 2 {
            return Err("array must have exactly 2 elements [x, y]".to_string());
        }
        let x = arr[0]
            .as_f64()
            .ok_or_else(|| "x must be numeric".to_string())?;
        let y = arr[1]
            .as_f64()
            .ok_or_else(|| "y must be numeric".to_string())?;
        if !x.is_finite() || !y.is_finite() {
            return Err("x and y must be finite numbers".to_string());
        }
        return Ok((x, y));
    }
    if let Some(obj) = v.as_object() {
        let x = obj
            .get("x")
            .and_then(Value::as_f64)
            .ok_or_else(|| "missing or non-numeric `x`".to_string())?;
        let y = obj
            .get("y")
            .and_then(Value::as_f64)
            .ok_or_else(|| "missing or non-numeric `y`".to_string())?;
        if !x.is_finite() || !y.is_finite() {
            return Err("x and y must be finite numbers".to_string());
        }
        return Ok((x, y));
    }
    Err("expected [x, y] pair or {x, y} object".to_string())
}

fn short_json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn render_summary(
    kind: ChartKind,
    title: Option<&str>,
    n_series: usize,
    n_labels: usize,
) -> String {
    let title = title.unwrap_or("(untitled)");
    match kind {
        ChartKind::Pie | ChartKind::Doughnut => format!(
            "{} chart \"{title}\" displayed in the desktop app with {n_labels} slices.",
            kind.as_str()
        ),
        ChartKind::Scatter => format!(
            "Scatter chart \"{title}\" displayed in the desktop app with {n_series} series."
        ),
        ChartKind::Line | ChartKind::Bar => format!(
            "{} chart \"{title}\" displayed in the desktop app with {n_series} series, {n_labels} points each.",
            kind.as_str()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_kind_parses_canonical_names() {
        assert_eq!(ChartKind::parse("line"), Some(ChartKind::Line));
        assert_eq!(ChartKind::parse("BAR"), Some(ChartKind::Bar));
        assert_eq!(ChartKind::parse("Pie"), Some(ChartKind::Pie));
        assert_eq!(ChartKind::parse("doughnut"), Some(ChartKind::Doughnut));
        assert_eq!(ChartKind::parse("donut"), Some(ChartKind::Doughnut));
        assert_eq!(ChartKind::parse("scatter"), Some(ChartKind::Scatter));
        assert_eq!(ChartKind::parse("area"), None);
    }

    #[test]
    fn parse_numeric_series_accepts_ints_and_floats() {
        let v = serde_json::json!([1, 2.5, -3, 0]);
        let out = parse_numeric_series(&v).unwrap();
        assert_eq!(out, vec![1.0, 2.5, -3.0, 0.0]);
    }

    #[test]
    fn parse_numeric_series_rejects_non_numeric() {
        let v = serde_json::json!([1, "two", 3]);
        let err = parse_numeric_series(&v).unwrap_err();
        assert!(err.contains("point #1"));
    }

    #[test]
    fn parse_numeric_series_rejects_non_array() {
        let v = serde_json::json!({"a": 1});
        assert!(parse_numeric_series(&v).is_err());
    }

    #[test]
    fn parse_scatter_series_accepts_array_pairs() {
        let v = serde_json::json!([[1, 2], [3.5, 4.5]]);
        let out = parse_scatter_series(&v).unwrap();
        assert_eq!(out, vec![(1.0, 2.0), (3.5, 4.5)]);
    }

    #[test]
    fn parse_scatter_series_accepts_xy_objects() {
        let v = serde_json::json!([{"x": 1, "y": 2}, {"x": 3, "y": 4}]);
        let out = parse_scatter_series(&v).unwrap();
        assert_eq!(out, vec![(1.0, 2.0), (3.0, 4.0)]);
    }

    #[test]
    fn parse_scatter_series_rejects_wrong_pair_length() {
        let v = serde_json::json!([[1, 2, 3]]);
        assert!(parse_scatter_series(&v).is_err());
    }

    #[test]
    fn build_payload_rejects_pie_with_negative_value() {
        let input = r#"{"type":"pie","labels":["A","B"],"series":[{"name":"x","data":[10,-5]}]}"#;
        let err = build_payload(input).unwrap_err();
        assert!(err.contains("cannot display negative values"));
    }

    #[test]
    fn build_payload_rejects_pie_with_multiple_series() {
        let input = r#"{"type":"pie","labels":["A"],"series":[{"name":"a","data":[1]},{"name":"b","data":[2]}]}"#;
        let err = build_payload(input).unwrap_err();
        assert!(err.contains("accept only one series"));
    }

    #[test]
    fn build_payload_rejects_mismatched_data_length() {
        let input = r#"{"type":"bar","labels":["A","B","C"],"series":[{"name":"x","data":[1,2]}]}"#;
        let err = build_payload(input).unwrap_err();
        assert!(err.contains("counts must match"));
    }

    #[test]
    fn build_payload_accepts_minimal_bar() {
        let input = r#"{"type":"bar","labels":["A","B"],"series":[{"name":"x","data":[1,2]}]}"#;
        let (payload, summary) = build_payload(input).unwrap();
        assert_eq!(payload["type"], "bar");
        assert!(summary.contains("bar chart"));
    }

    #[test]
    fn build_payload_normalises_scatter_pairs_to_xy_objects() {
        let input = r#"{"type":"scatter","series":[{"name":"s","data":[[1,2],[3,4]]}]}"#;
        let (payload, _) = build_payload(input).unwrap();
        let pts = payload["series"][0]["data"].as_array().unwrap();
        assert_eq!(pts[0]["x"], 1.0);
        assert_eq!(pts[0]["y"], 2.0);
        assert_eq!(pts[1]["x"], 3.0);
        assert_eq!(pts[1]["y"], 4.0);
    }

    #[test]
    fn cli_role_refuses_with_do_not_retry_hint() {
        // Default role is Cli — make sure the gate fires with a clear
        // "do not retry" message so a model doesn't loop.
        let out =
            tool_draw_chart(r#"{"type":"bar","labels":["A"],"series":[{"name":"x","data":[1]}]}"#);
        assert!(out.contains("only available in the aictl desktop app"));
        assert!(out.contains("Do not retry"));
    }

    #[test]
    fn cli_role_empty_input_still_gated() {
        // The role gate fires before parsing — same message regardless
        // of input shape. This protects models from getting a parse
        // error and "fixing" the input only to hit the role gate
        // anyway.
        let out = tool_draw_chart("");
        assert!(out.contains("only available in the aictl desktop app"));
    }
}
