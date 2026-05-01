//! Provider health check (`/ping`).
//!
//! Probes every cloud provider catalog endpoint plus the local Ollama daemon
//! and prints a per-provider status line with elapsed latency. For cloud
//! providers the probe is a minimal authenticated `GET /models` call which
//! validates that the configured API key is accepted without consuming any
//! completion tokens. Providers without a configured key are reported as
//! "no API key" and skipped. GGUF and MLX are skipped — they are local,
//! file-system–backed providers with no connectivity to verify.

use std::time::{Duration, Instant};

use crossterm::style::{Color, Stylize};
use futures_util::future::join_all;

use crate::keys;

/// Per-provider probe timeout. Long enough to ride out a slow TLS handshake
/// on a congested link, short enough that a dead endpoint doesn't stall the
/// whole sweep.
const PING_TIMEOUT: Duration = Duration::from_secs(10);

/// Cloud providers probed by `/ping`, in display order. Each entry maps the
/// provider's display name to the config/keyring key name used to fetch its
/// API secret. The actual endpoint + auth scheme lives in [`probe_request`].
const CLOUD_PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "LLM_ANTHROPIC_API_KEY"),
    ("openai", "LLM_OPENAI_API_KEY"),
    ("gemini", "LLM_GEMINI_API_KEY"),
    ("grok", "LLM_GROK_API_KEY"),
    ("mistral", "LLM_MISTRAL_API_KEY"),
    ("deepseek", "LLM_DEEPSEEK_API_KEY"),
    ("kimi", "LLM_KIMI_API_KEY"),
    ("zai", "LLM_ZAI_API_KEY"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PingStatus {
    Ok,
    NoKey,
    Fail,
    NotRunning,
}

struct PingResult {
    provider: &'static str,
    status: PingStatus,
    detail: String,
    elapsed: Option<Duration>,
}

/// Run the full `/ping` sweep and print results to stdout.
pub async fn run_ping() {
    println!();
    println!("  {} pinging providers...", "↻".with(Color::Cyan),);
    println!();

    let cloud_futures = CLOUD_PROVIDERS.iter().map(|&(name, key_name)| async move {
        match keys::get_secret(key_name) {
            Some(key) if !key.is_empty() => probe_cloud(name, &key).await,
            _ => PingResult {
                provider: name,
                status: PingStatus::NoKey,
                detail: "no API key".to_string(),
                elapsed: None,
            },
        }
    });

    let (cloud_results, ollama_result, aictl_server_result) = tokio::join!(
        join_all(cloud_futures),
        probe_ollama(),
        probe_aictl_server(),
    );

    let mut all: Vec<PingResult> = cloud_results;
    all.push(ollama_result);
    all.push(aictl_server_result);

    let max_name = all.iter().map(|r| r.provider.len()).max().unwrap_or(0);
    let max_detail = all.iter().map(|r| r.detail.len()).max().unwrap_or(0);

    for r in &all {
        print_result(r, max_name, max_detail);
    }

    let ok = all.iter().filter(|r| r.status == PingStatus::Ok).count();
    let fail = all
        .iter()
        .filter(|r| matches!(r.status, PingStatus::Fail | PingStatus::NotRunning))
        .count();
    let skipped = all.iter().filter(|r| r.status == PingStatus::NoKey).count();
    println!();
    println!(
        "  {} {ok} ok · {fail} fail · {skipped} no key",
        "summary:".with(Color::Cyan),
    );
    println!();
}

async fn probe_cloud(name: &'static str, key: &str) -> PingResult {
    let start = Instant::now();
    let req = probe_request(name, key);
    match req.timeout(PING_TIMEOUT).send().await {
        Ok(resp) => {
            let elapsed = start.elapsed();
            let status = resp.status();
            if status.is_success() {
                PingResult {
                    provider: name,
                    status: PingStatus::Ok,
                    detail: format!("HTTP {}", status.as_u16()),
                    elapsed: Some(elapsed),
                }
            } else {
                PingResult {
                    provider: name,
                    status: PingStatus::Fail,
                    detail: format!("HTTP {}", status.as_u16()),
                    elapsed: Some(elapsed),
                }
            }
        }
        Err(e) => {
            let detail = if e.is_timeout() {
                "timeout".to_string()
            } else if e.is_connect() {
                "connect failed".to_string()
            } else {
                "error".to_string()
            };
            PingResult {
                provider: name,
                status: PingStatus::Fail,
                detail,
                elapsed: None,
            }
        }
    }
}

fn probe_request(name: &str, key: &str) -> reqwest::RequestBuilder {
    let client = crate::config::http_client();
    match name {
        "openai" => client
            .get("https://api.openai.com/v1/models")
            .header("Authorization", format!("Bearer {key}")),
        "anthropic" => client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        "gemini" => client.get(format!(
            "https://generativelanguage.googleapis.com/v1beta/models?key={key}"
        )),
        "grok" => client
            .get("https://api.x.ai/v1/models")
            .header("Authorization", format!("Bearer {key}")),
        "mistral" => client
            .get("https://api.mistral.ai/v1/models")
            .header("Authorization", format!("Bearer {key}")),
        "deepseek" => client
            .get("https://api.deepseek.com/models")
            .header("Authorization", format!("Bearer {key}")),
        "kimi" => client
            .get("https://api.moonshot.ai/v1/models")
            .header("Authorization", format!("Bearer {key}")),
        "zai" => client
            .get("https://api.z.ai/api/paas/v4/models")
            .header("Authorization", format!("Bearer {key}")),
        _ => unreachable!("unknown provider {name} in probe_request"),
    }
}

/// Probe the configured `aictl-server` (`AICTL_CLIENT_HOST`) — `GET /healthz`
/// is auth-free, so this works whether or not the master key is set, but
/// when both URL and key are present we additionally verify the master
/// key is accepted by hitting the authenticated `/v1/models` route.
async fn probe_aictl_server() -> PingResult {
    let Some(url) = crate::config::client_url() else {
        return PingResult {
            provider: "aictl-server",
            status: PingStatus::NoKey,
            detail: "AICTL_CLIENT_HOST not set".to_string(),
            elapsed: None,
        };
    };
    let client = crate::config::http_client();
    let start = Instant::now();
    let healthz = format!("{}/healthz", url.trim_end_matches('/'));
    let resp = match client.get(&healthz).timeout(PING_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => {
            let detail = if e.is_timeout() {
                "timeout".to_string()
            } else if e.is_connect() {
                "connect failed".to_string()
            } else {
                "error".to_string()
            };
            return PingResult {
                provider: "aictl-server",
                status: PingStatus::NotRunning,
                detail,
                elapsed: None,
            };
        }
    };
    if !resp.status().is_success() {
        return PingResult {
            provider: "aictl-server",
            status: PingStatus::Fail,
            detail: format!("HTTP {}", resp.status().as_u16()),
            elapsed: Some(start.elapsed()),
        };
    }
    // Healthz passed. If a master key is configured, also exercise the
    // authenticated path so a stale/wrong key surfaces as a clear fail
    // rather than waiting until the user runs a chat call.
    if let Some(master_key) = keys::get_secret("AICTL_CLIENT_MASTER_KEY")
        && !master_key.is_empty()
    {
        let auth_url = format!("{}/v1/models", url.trim_end_matches('/'));
        match client
            .get(&auth_url)
            .header("Authorization", format!("Bearer {master_key}"))
            .timeout(PING_TIMEOUT)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => PingResult {
                provider: "aictl-server",
                status: PingStatus::Ok,
                detail: "running, key accepted".to_string(),
                elapsed: Some(start.elapsed()),
            },
            Ok(r) => PingResult {
                provider: "aictl-server",
                status: PingStatus::Fail,
                detail: format!("auth HTTP {}", r.status().as_u16()),
                elapsed: Some(start.elapsed()),
            },
            Err(_) => PingResult {
                provider: "aictl-server",
                status: PingStatus::Fail,
                detail: "auth probe failed".to_string(),
                elapsed: Some(start.elapsed()),
            },
        }
    } else {
        PingResult {
            provider: "aictl-server",
            status: PingStatus::Ok,
            detail: "running (no master key set)".to_string(),
            elapsed: Some(start.elapsed()),
        }
    }
}

async fn probe_ollama() -> PingResult {
    let start = Instant::now();
    let base = crate::config::config_get("LLM_OLLAMA_HOST")
        .unwrap_or_else(|| "http://localhost:11434".to_string());
    let url = format!("{base}/api/tags");
    let client = crate::config::http_client();
    match client.get(&url).timeout(PING_TIMEOUT).send().await {
        Ok(resp) if resp.status().is_success() => PingResult {
            provider: "ollama",
            status: PingStatus::Ok,
            detail: "running".to_string(),
            elapsed: Some(start.elapsed()),
        },
        _ => PingResult {
            provider: "ollama",
            status: PingStatus::NotRunning,
            detail: "not running".to_string(),
            elapsed: None,
        },
    }
}

fn print_result(r: &PingResult, max_name: usize, max_detail: usize) {
    let (icon, color) = match r.status {
        PingStatus::Ok => ("✓", Color::Green),
        PingStatus::NoKey | PingStatus::NotRunning => ("-", Color::DarkGrey),
        PingStatus::Fail => ("✗", Color::Red),
    };
    let elapsed = r
        .elapsed
        .map(|d| format!("{}ms", d.as_millis()))
        .unwrap_or_default();
    let name_pad = max_name - r.provider.len() + 2;
    let detail_pad = max_detail - r.detail.len() + 2;
    println!(
        "  {} {}{:name_pad$}{}{:detail_pad$}{}",
        icon.with(color),
        r.provider.with(Color::Cyan),
        "",
        r.detail.clone().with(color),
        "",
        elapsed.with(Color::DarkGrey),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_providers_match_key_names() {
        for &(_, key_name) in CLOUD_PROVIDERS {
            assert!(
                keys::KEY_NAMES.contains(&key_name),
                "{key_name} missing from keys::KEY_NAMES"
            );
        }
    }

    #[test]
    fn cloud_providers_match_model_catalog() {
        use std::collections::HashSet;
        let catalog: HashSet<&str> = crate::llm::MODELS.iter().map(|&(p, _, _)| p).collect();
        let probed: HashSet<&str> = CLOUD_PROVIDERS.iter().map(|&(p, _)| p).collect();
        assert_eq!(
            catalog, probed,
            "CLOUD_PROVIDERS must match the set of cloud providers in llm::MODELS"
        );
    }
}
