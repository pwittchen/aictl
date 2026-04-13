use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stats recorded for a single day.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DayStats {
    pub requests: u64,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// model name -> number of requests using that model
    pub models: HashMap<String, u64>,
}

impl DayStats {
    fn merge(&mut self, other: &Self) {
        self.requests += other.requests;
        self.llm_calls += other.llm_calls;
        self.tool_calls += other.tool_calls;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cost_usd += other.cost_usd;
        for (model, count) in &other.models {
            *self.models.entry(model.clone()).or_default() += count;
        }
    }
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

fn stats_dir() -> Option<PathBuf> {
    let h = home()?;
    let p = PathBuf::from(format!("{h}/.aictl/stats"));
    let _ = fs::create_dir_all(&p);
    Some(p)
}

fn today_key() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Convert epoch seconds to YYYY-MM-DD
    let days = secs / 86400;
    let (y, m, d) = epoch_days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn this_month_prefix() -> String {
    let key = today_key();
    // "YYYY-MM-DD" -> "YYYY-MM"
    key[..7].to_string()
}

/// Convert days since epoch to (year, month, day).
fn epoch_days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Civil calendar algorithm from Howard Hinnant
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn day_file(key: &str) -> Option<PathBuf> {
    Some(stats_dir()?.join(format!("{key}.json")))
}

fn load_day(key: &str) -> DayStats {
    let Some(path) = day_file(key) else {
        return DayStats::default();
    };
    let Ok(data) = fs::read_to_string(&path) else {
        return DayStats::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_day(key: &str, stats: &DayStats) {
    let Some(path) = day_file(key) else {
        return;
    };
    let _ = fs::write(
        &path,
        serde_json::to_string_pretty(stats).unwrap_or_default(),
    );
}

/// Record stats from a completed agent turn.
#[allow(clippy::cast_precision_loss)]
pub fn record(model: &str, llm_calls: u32, tool_calls: u32, usage: &crate::llm::TokenUsage) {
    let key = today_key();
    let mut day = load_day(&key);
    day.requests += 1;
    day.llm_calls += u64::from(llm_calls);
    day.tool_calls += u64::from(tool_calls);
    day.input_tokens += usage.input_tokens;
    day.output_tokens += usage.output_tokens;
    day.cost_usd += usage.estimate_cost(model).unwrap_or(0.0);
    *day.models.entry(model.to_string()).or_default() += 1;
    save_day(&key, &day);
}

/// Load today's stats.
pub fn today() -> DayStats {
    load_day(&today_key())
}

/// Load this month's aggregated stats.
pub fn this_month() -> DayStats {
    let prefix = this_month_prefix();
    aggregate_with_prefix(&prefix)
}

/// Load overall aggregated stats across all days.
pub fn overall() -> DayStats {
    aggregate_with_prefix("")
}

fn aggregate_with_prefix(prefix: &str) -> DayStats {
    let Some(dir) = stats_dir() else {
        return DayStats::default();
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return DayStats::default();
    };
    let mut agg = DayStats::default();
    for entry in rd.flatten() {
        let fname = entry.file_name().to_string_lossy().into_owned();
        let Some(key) = fname.strip_suffix(".json") else {
            continue;
        };
        if !key.starts_with(prefix) {
            continue;
        }
        let day = load_day(key);
        agg.merge(&day);
    }
    agg
}

/// Remove all stats files.
pub fn clear_all() {
    let Some(dir) = stats_dir() else {
        return;
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Count how many day files exist.
pub fn day_count() -> usize {
    let Some(dir) = stats_dir() else {
        return 0;
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_days_to_ymd_unix_epoch() {
        let (y, m, d) = epoch_days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn epoch_days_to_ymd_known_date() {
        // 2026-04-13 is day 20_556 since epoch
        let (y, m, d) = epoch_days_to_ymd(20_556);
        assert_eq!((y, m, d), (2026, 4, 13));
    }

    #[test]
    fn day_stats_merge() {
        let mut a = DayStats {
            requests: 5,
            llm_calls: 10,
            tool_calls: 3,
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.05,
            models: HashMap::from([
                ("gpt-4o".to_string(), 3),
                ("claude-sonnet-4-20250514".to_string(), 2),
            ]),
        };
        let b = DayStats {
            requests: 2,
            llm_calls: 4,
            tool_calls: 1,
            input_tokens: 400,
            output_tokens: 200,
            cost_usd: 0.02,
            models: HashMap::from([("gpt-4o".to_string(), 1), ("gpt-4.1".to_string(), 1)]),
        };
        a.merge(&b);
        assert_eq!(a.requests, 7);
        assert_eq!(a.llm_calls, 14);
        assert_eq!(a.tool_calls, 4);
        assert_eq!(a.input_tokens, 1400);
        assert_eq!(a.output_tokens, 700);
        assert!((a.cost_usd - 0.07).abs() < 1e-9);
        assert_eq!(a.models["gpt-4o"], 4);
        assert_eq!(a.models["claude-sonnet-4-20250514"], 2);
        assert_eq!(a.models["gpt-4.1"], 1);
    }
}
