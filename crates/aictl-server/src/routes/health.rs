use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
    version: &'static str,
    uptime_secs: u64,
    active_requests: usize,
}

pub async fn healthz(State(state): State<Arc<AppState>>) -> Json<Health> {
    let active = state
        .config
        .max_concurrent_requests
        .saturating_sub(state.semaphore.available_permits());
    Json(Health {
        status: "ok",
        version: aictl_core::VERSION,
        uptime_secs: state.started_at.elapsed().as_secs(),
        active_requests: active,
    })
}
