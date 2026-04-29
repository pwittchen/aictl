use axum::Json;
use serde::Serialize;

use aictl_core::stats::{DayStats, overall, this_month, today};

#[derive(Serialize)]
pub struct StatsResponse {
    today: DayStats,
    this_month: DayStats,
    overall: DayStats,
}

pub async fn stats() -> Json<StatsResponse> {
    Json(StatsResponse {
        today: today(),
        this_month: this_month(),
        overall: overall(),
    })
}
