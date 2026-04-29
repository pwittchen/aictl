//! Shared server state: master key, semaphore for the global
//! concurrency cap, immutable [`ServerConfig`], and a startup
//! timestamp surfaced by `/healthz`.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;

use crate::config::ServerConfig;
use crate::rate_limit::RateLimiter;

pub struct AppState {
    pub master_key: String,
    pub config: ServerConfig,
    pub semaphore: Arc<Semaphore>,
    /// `None` when `AICTL_SERVER_RATE_LIMIT_RPM=0` (the default).
    /// The middleware short-circuits when no limiter is installed.
    pub rate_limiter: Option<Arc<RateLimiter>>,
    pub started_at: Instant,
}

impl AppState {
    #[must_use]
    pub fn new(master_key: String, config: ServerConfig) -> Arc<Self> {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_requests));
        let rate_limiter =
            RateLimiter::new(config.rate_limit_rpm, config.rate_limit_burst).map(Arc::new);
        Arc::new(Self {
            master_key,
            config,
            semaphore,
            rate_limiter,
            started_at: Instant::now(),
        })
    }
}
