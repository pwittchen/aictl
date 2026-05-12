//! Provider health-check command — desktop counterpart to the CLI's
//! `/ping`. Probes every cloud provider catalog endpoint plus the local
//! Ollama daemon and the configured `aictl-server`. The webview renders
//! the result as a modal with a green check or red x per provider plus
//! the elapsed latency.

use std::time::{Duration, Instant};

use aictl_core::{config, keys};
use futures_util::future::join_all;
use serde::Serialize;

const PING_TIMEOUT: Duration = Duration::from_secs(10);

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

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum PingStatus {
    Ok,
    NoKey,
    Fail,
    NotRunning,
}

#[derive(Serialize)]
pub struct PingResult {
    pub provider: String,
    pub status: PingStatus,
    pub detail: String,
    pub elapsed_ms: Option<u64>,
}

#[tauri::command]
pub async fn ping_providers() -> Vec<PingResult> {
    let cloud_futures = CLOUD_PROVIDERS.iter().map(|&(name, key_name)| async move {
        match keys::get_secret(key_name) {
            Some(key) if !key.is_empty() => probe_cloud(name, &key).await,
            _ => PingResult {
                provider: name.to_string(),
                status: PingStatus::NoKey,
                detail: "no API key".to_string(),
                elapsed_ms: None,
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
    all
}

async fn probe_cloud(name: &'static str, key: &str) -> PingResult {
    let start = Instant::now();
    let client = config::http_client();
    // Two reqwest versions live in this crate's tree (core ships 0.13,
    // tauri pulls in 0.12). Inlining the request keeps inference on
    // core's 0.13 `RequestBuilder` without a cross-version cast.
    let req = match name {
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
        _ => unreachable!("unknown provider {name} in probe_cloud"),
    };
    match req.timeout(PING_TIMEOUT).send().await {
        Ok(resp) => {
            let elapsed = start.elapsed();
            let status = resp.status();
            let (s, detail) = if status.is_success() {
                (PingStatus::Ok, format!("HTTP {}", status.as_u16()))
            } else {
                (PingStatus::Fail, format!("HTTP {}", status.as_u16()))
            };
            PingResult {
                provider: name.to_string(),
                status: s,
                detail,
                elapsed_ms: Some(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)),
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
                provider: name.to_string(),
                status: PingStatus::Fail,
                detail,
                elapsed_ms: None,
            }
        }
    }
}

async fn probe_aictl_server() -> PingResult {
    let Some(url) = config::client_url() else {
        return PingResult {
            provider: "aictl-server".to_string(),
            status: PingStatus::NoKey,
            detail: "AICTL_CLIENT_HOST not set".to_string(),
            elapsed_ms: None,
        };
    };
    let client = config::http_client();
    let start = Instant::now();
    let healthz = format!("{}/healthz", url.trim_end_matches('/'));
    let resp = match client.get(&healthz).timeout(PING_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => {
            let detail = if e.is_timeout() {
                "timeout".to_string()
            } else if e.is_connect() {
                "not running".to_string()
            } else {
                "error".to_string()
            };
            return PingResult {
                provider: "aictl-server".to_string(),
                status: PingStatus::NotRunning,
                detail,
                elapsed_ms: None,
            };
        }
    };
    if !resp.status().is_success() {
        return PingResult {
            provider: "aictl-server".to_string(),
            status: PingStatus::Fail,
            detail: format!("HTTP {}", resp.status().as_u16()),
            elapsed_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
        };
    }
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
                provider: "aictl-server".to_string(),
                status: PingStatus::Ok,
                detail: "running, key accepted".to_string(),
                elapsed_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
            },
            Ok(r) => PingResult {
                provider: "aictl-server".to_string(),
                status: PingStatus::Fail,
                detail: format!("auth HTTP {}", r.status().as_u16()),
                elapsed_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
            },
            Err(_) => PingResult {
                provider: "aictl-server".to_string(),
                status: PingStatus::Fail,
                detail: "auth probe failed".to_string(),
                elapsed_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
            },
        }
    } else {
        PingResult {
            provider: "aictl-server".to_string(),
            status: PingStatus::Ok,
            detail: "running (no master key set)".to_string(),
            elapsed_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
        }
    }
}

async fn probe_ollama() -> PingResult {
    let start = Instant::now();
    let base = config::config_get("LLM_OLLAMA_HOST")
        .unwrap_or_else(|| "http://localhost:11434".to_string());
    let url = format!("{base}/api/tags");
    let client = config::http_client();
    match client.get(&url).timeout(PING_TIMEOUT).send().await {
        Ok(resp) if resp.status().is_success() => PingResult {
            provider: "ollama".to_string(),
            status: PingStatus::Ok,
            detail: "running".to_string(),
            elapsed_ms: Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
        },
        _ => PingResult {
            provider: "ollama".to_string(),
            status: PingStatus::NotRunning,
            detail: "not running".to_string(),
            elapsed_ms: None,
        },
    }
}
