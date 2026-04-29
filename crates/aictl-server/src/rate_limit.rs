//! Per-client token-bucket rate limiter.
//!
//! Configurable via `AICTL_SERVER_RATE_LIMIT_RPM` (requests per minute,
//! `0` disables) and `AICTL_SERVER_RATE_LIMIT_BURST` (max consecutive
//! requests; defaults to the RPM value when `0` so the bucket holds
//! one minute's worth of tokens). Buckets are keyed by client IP.
//!
//! Plan §6 covers the global concurrency cap (a `tokio::Semaphore`);
//! this is the second tier — a per-IP token bucket on top of the
//! semaphore. Saturation surfaces as 429 with a `Retry-After` header.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Soft cap on the per-IP bucket map; opportunistic cleanup runs when
/// the count exceeds this threshold to prevent unbounded growth on a
/// host fielding many distinct client IPs.
const MAX_BUCKETS: usize = 10_000;

/// How long an idle bucket has to sit before it's eligible for GC.
/// Two minutes is comfortably longer than any reasonable burst-recovery
/// window — by then the bucket has refilled to capacity anyway, so
/// reconstructing it on the next request is a no-op.
const IDLE_TTL: Duration = Duration::from_secs(120);

#[derive(Debug)]
struct TokenBucket {
    capacity: f64,
    rate_per_sec: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    fn new(capacity: f64, rate_per_sec: f64, now: Instant) -> Self {
        Self {
            capacity,
            rate_per_sec,
            tokens: capacity,
            last: now,
        }
    }

    /// Refill based on elapsed wall time, then attempt to consume
    /// one token. On success returns `Ok(())`; on failure returns the
    /// estimated wait until one full token is available.
    fn try_consume(&mut self, now: Instant) -> Result<(), Duration> {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(());
        }
        let needed = 1.0 - self.tokens;
        let secs = needed / self.rate_per_sec;
        Err(Duration::from_secs_f64(secs.max(1.0)))
    }
}

/// Per-IP rate limiter. Cheap to clone via `Arc<RateLimiter>`; all
/// state lives behind a single `Mutex<HashMap<...>>`.
pub struct RateLimiter {
    capacity: f64,
    rate_per_sec: f64,
    buckets: Mutex<HashMap<String, TokenBucket>>,
}

impl RateLimiter {
    /// Build a limiter for the given requests-per-minute and burst
    /// capacity. Returns `None` when `rpm == 0` so callers can skip
    /// the layer entirely instead of installing a no-op.
    #[must_use]
    pub fn new(rpm: u32, burst: u32) -> Option<Self> {
        if rpm == 0 {
            return None;
        }
        let capacity = if burst == 0 {
            f64::from(rpm)
        } else {
            f64::from(burst)
        };
        let rate_per_sec = f64::from(rpm) / 60.0;
        Some(Self {
            capacity,
            rate_per_sec,
            buckets: Mutex::new(HashMap::new()),
        })
    }

    /// Check (and consume) one token for `key`. On 429, the returned
    /// `Duration` is the suggested `Retry-After` window.
    pub fn check(&self, key: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| {
            // Poisoning means a previous holder panicked; we can still
            // serve traffic by clearing the map under the recovered guard.
            let mut guard = e.into_inner();
            guard.clear();
            guard
        });

        if buckets.len() > MAX_BUCKETS {
            buckets.retain(|_, b| now.saturating_duration_since(b.last) < IDLE_TTL);
        }

        let bucket = buckets
            .entry(key.to_string())
            .or_insert_with(|| TokenBucket::new(self.capacity, self.rate_per_sec, now));
        bucket.try_consume(now)
    }

    /// Number of buckets currently held. Test-only — the live
    /// counter on the running server is not interesting enough to
    /// expose elsewhere.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.buckets.lock().map(|m| m.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_when_rpm_is_zero() {
        assert!(RateLimiter::new(0, 0).is_none());
        assert!(RateLimiter::new(0, 100).is_none());
    }

    #[test]
    fn first_n_requests_pass_within_burst() {
        let limiter = RateLimiter::new(60, 5).unwrap();
        for _ in 0..5 {
            assert!(limiter.check("1.2.3.4").is_ok());
        }
    }

    #[test]
    fn exhausting_burst_returns_retry_after() {
        let limiter = RateLimiter::new(60, 3).unwrap();
        assert!(limiter.check("1.2.3.4").is_ok());
        assert!(limiter.check("1.2.3.4").is_ok());
        assert!(limiter.check("1.2.3.4").is_ok());
        let err = limiter.check("1.2.3.4").unwrap_err();
        // 60 rpm = 1 token/sec, so the next token is ~1s away.
        assert!(err.as_secs() >= 1);
    }

    #[test]
    fn separate_keys_have_separate_buckets() {
        let limiter = RateLimiter::new(60, 1).unwrap();
        assert!(limiter.check("1.2.3.4").is_ok());
        // Bucket for 1.2.3.4 is empty now, but a fresh key starts full.
        assert!(limiter.check("5.6.7.8").is_ok());
        assert!(limiter.check("1.2.3.4").is_err());
    }

    #[test]
    fn refill_replenishes_tokens_over_time() {
        // 600 rpm = 10 tokens/sec; one token in 100ms.
        let limiter = RateLimiter::new(600, 1).unwrap();
        assert!(limiter.check("k").is_ok());
        assert!(limiter.check("k").is_err());
        std::thread::sleep(Duration::from_millis(150));
        assert!(limiter.check("k").is_ok());
    }

    #[test]
    fn burst_defaults_to_rpm_when_zero() {
        let limiter = RateLimiter::new(120, 0).unwrap();
        // capacity should be 120 tokens.
        for _ in 0..120 {
            assert!(limiter.check("k").is_ok());
        }
        assert!(limiter.check("k").is_err());
    }

    #[test]
    fn buckets_count_grows_with_distinct_keys() {
        let limiter = RateLimiter::new(60, 1).unwrap();
        for i in 0..50 {
            let _ = limiter.check(&format!("ip-{i}"));
        }
        assert_eq!(limiter.len(), 50);
    }
}
