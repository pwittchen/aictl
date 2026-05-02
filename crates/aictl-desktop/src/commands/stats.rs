//! Stats-pane Tauri commands.
//!
//! Surfaces the same `~/.aictl/stats/<day>.json` aggregates the CLI
//! prints from `/stats`. Today / month / overall.

use aictl_core::stats;
use serde::Serialize;

#[derive(Serialize)]
pub struct StatsBucket {
    pub label: &'static str,
    pub sessions: u64,
    pub requests: u64,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// Sorted descending by request count.
    pub models: Vec<ModelRow>,
}

#[derive(Serialize)]
pub struct ModelRow {
    pub model: String,
    pub count: u64,
}

#[derive(Serialize)]
pub struct StatsSnapshot {
    pub day_count: usize,
    pub today: StatsBucket,
    pub month: StatsBucket,
    pub overall: StatsBucket,
}

#[tauri::command]
pub fn stats_snapshot() -> StatsSnapshot {
    StatsSnapshot {
        day_count: stats::day_count(),
        today: project("today", &stats::today()),
        month: project("month", &stats::this_month()),
        overall: project("overall", &stats::overall()),
    }
}

#[tauri::command]
pub fn stats_clear() -> Result<(), String> {
    stats::clear_all();
    Ok(())
}

fn project(label: &'static str, day: &stats::DayStats) -> StatsBucket {
    let mut models: Vec<ModelRow> = day
        .models
        .iter()
        .map(|(m, c)| ModelRow {
            model: m.clone(),
            count: *c,
        })
        .collect();
    models.sort_by(|a, b| b.count.cmp(&a.count));
    StatsBucket {
        label,
        sessions: day.sessions,
        requests: day.requests,
        llm_calls: day.llm_calls,
        tool_calls: day.tool_calls,
        input_tokens: day.input_tokens,
        output_tokens: day.output_tokens,
        cost_usd: day.cost_usd,
        models,
    }
}
