//! Cached remote version check.
//!
//! On interactive startup, aictl checks the latest upstream version via
//! [`crate::fetch_remote_version`] and saves the result plus the current
//! timestamp to `~/.aictl/version`. Subsequent startups within 24 hours
//! read the cached value instead of hitting the network, so the welcome
//! banner shows the version notice instantly and the user is never
//! stalled by an upstream hiccup. Once the cached entry ages past
//! [`CACHE_TTL_SECS`], the next startup performs a fresh fetch and
//! rewrites the file with the new version and timestamp.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Reuse a cached remote version this many seconds before re-fetching.
pub const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct VersionCache {
    version: String,
    /// Unix timestamp (seconds since epoch) when the check was performed.
    checked_at: u64,
}

fn cache_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(format!("{h}/.aictl/version")))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_fresh_at(path: &Path, now: u64) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let cache: VersionCache = serde_json::from_str(&contents).ok()?;
    let age = now.saturating_sub(cache.checked_at);
    if age < CACHE_TTL_SECS {
        Some(cache.version)
    } else {
        None
    }
}

fn write_at(path: &Path, version: &str, now: u64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let cache = VersionCache {
        version: version.to_string(),
        checked_at: now,
    };
    if let Ok(contents) = serde_json::to_string(&cache) {
        let _ = std::fs::write(path, contents);
    }
}

/// Return the cached remote version if the cache file exists and is less
/// than [`CACHE_TTL_SECS`] old. Returns `None` on any read / parse error
/// or when the entry is stale, signaling a fresh fetch is due.
pub fn cached_fresh() -> Option<String> {
    read_fresh_at(&cache_path()?, now_secs())
}

/// Persist a freshly-fetched remote version to `~/.aictl/version` with the
/// current timestamp. Errors (missing HOME, read-only disk, serde hiccup)
/// are swallowed — the cache is a best-effort optimization.
pub fn save(version: &str) {
    let Some(path) = cache_path() else { return };
    write_at(&path, version, now_secs());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_within_ttl_is_returned() {
        let dir = std::env::temp_dir().join(format!("aictl-vc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("version");
        let now = 1_700_000_000;
        write_at(&path, "1.2.3", now);
        assert_eq!(read_fresh_at(&path, now + 60), Some("1.2.3".to_string()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_past_ttl_is_none() {
        let dir = std::env::temp_dir().join(format!("aictl-vc-stale-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("version");
        let now = 1_700_000_000;
        write_at(&path, "1.2.3", now);
        assert_eq!(read_fresh_at(&path, now + CACHE_TTL_SECS + 1), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_is_none() {
        let path = std::env::temp_dir().join("aictl-vc-does-not-exist");
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_fresh_at(&path, 0), None);
    }

    #[test]
    fn corrupt_file_is_none() {
        let dir = std::env::temp_dir().join(format!("aictl-vc-corrupt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("version");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_fresh_at(&path, 0), None);
        let _ = std::fs::remove_file(&path);
    }
}
