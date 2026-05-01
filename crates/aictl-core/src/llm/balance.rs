//! Provider credit / quota balance probes.
//!
//! For each cloud provider, expose a single async `fetch` that returns a
//! [`Balance`] describing the remaining headroom on the configured API key.
//!
//! Strategy per provider:
//!   * Real billing endpoint when one exists (`DeepSeek`, Kimi). Returns a
//!     concrete monetary amount.
//!   * `Unknown` with a short hint pointing at the provider's billing
//!     dashboard, when no public balance API is documented (`OpenAI`,
//!     `Gemini`, Grok, Mistral, Z.ai).
//!   * Anthropic: `Unknown` for the same reason — `usage_report` requires an
//!     Admin API key (a different secret than the chat key) and only reports
//!     historical spend, not remaining prepaid credit.
//!
//! Local providers (Ollama, GGUF, MLX) have no balance to report and are
//! omitted from the probe set.

use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::Deserialize;

use crate::keys;

const BALANCE_TIMEOUT: Duration = Duration::from_secs(10);

/// Cloud providers probed by `/balance`, in display order. Each entry maps the
/// provider's display name to the config/keyring key name used to fetch its
/// API secret.
pub const BALANCE_PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "LLM_ANTHROPIC_API_KEY"),
    ("openai", "LLM_OPENAI_API_KEY"),
    ("gemini", "LLM_GEMINI_API_KEY"),
    ("grok", "LLM_GROK_API_KEY"),
    ("mistral", "LLM_MISTRAL_API_KEY"),
    ("deepseek", "LLM_DEEPSEEK_API_KEY"),
    ("kimi", "LLM_KIMI_API_KEY"),
    ("zai", "LLM_ZAI_API_KEY"),
];

/// Result of a balance probe for a single provider.
#[derive(Debug, Clone)]
pub struct Balance {
    pub provider: &'static str,
    pub status: BalanceStatus,
    pub elapsed: Option<Duration>,
}

#[derive(Debug, Clone)]
pub enum BalanceStatus {
    /// Real monetary balance reported by the provider.
    Amount {
        currency: String,
        /// Available-to-spend now. For `DeepSeek` and Kimi this is the
        /// authoritative figure the API reports — `granted` / `topped_up`
        /// are informational breakdowns, not separate spendable amounts.
        total: f64,
        /// Promotional / granted credit (subset of `total`).
        granted: Option<f64>,
        /// User-funded top-up (subset of `total`).
        topped_up: Option<f64>,
    },
    /// API key not configured for this provider.
    NoKey,
    /// Provider does not expose a public balance endpoint. The string is a
    /// short hint pointing at the provider's billing dashboard.
    Unknown(String),
    /// Probe failed (network error, auth fail, malformed response).
    Error(String),
}

/// Probe every provider in `BALANCE_PROVIDERS` concurrently.
///
/// `aictl-server` is intentionally not probed here: the server's
/// `/v1/stats` reports dispatch counts, not upstream balances, and
/// mixing those two figures into the same table is misleading. Pick
/// `--provider aictl-server` (or `/model`) to route LLM traffic
/// through the server; balance probes always read direct upstream
/// endpoints with the operator's local keys.
pub async fn fetch_all() -> Vec<Balance> {
    let futures = BALANCE_PROVIDERS
        .iter()
        .map(|&(name, key_name)| async move {
            let key = keys::get_secret(key_name).filter(|k| !k.is_empty());
            fetch_one(name, key.as_deref()).await
        });
    join_all(futures).await
}

/// Probe a single provider by name. Returns `NoKey` when `api_key` is `None`,
/// otherwise dispatches to the provider-specific fetcher.
pub async fn fetch_one(name: &'static str, api_key: Option<&str>) -> Balance {
    let Some(api_key) = api_key else {
        return Balance {
            provider: name,
            status: BalanceStatus::NoKey,
            elapsed: None,
        };
    };
    let start = Instant::now();
    let status = match name {
        "deepseek" => fetch_deepseek(api_key).await,
        "kimi" => fetch_kimi(api_key).await,
        "anthropic" => BalanceStatus::Unknown(
            "no public balance API — check console.anthropic.com/settings/billing".to_string(),
        ),
        "openai" => BalanceStatus::Unknown(
            "no public balance API — check platform.openai.com/account/billing".to_string(),
        ),
        "gemini" => BalanceStatus::Unknown(
            "no public balance API — check console.cloud.google.com/billing".to_string(),
        ),
        "grok" => BalanceStatus::Unknown("no public balance API — check console.x.ai".to_string()),
        "mistral" => {
            BalanceStatus::Unknown("no public balance API — check console.mistral.ai".to_string())
        }
        "zai" => BalanceStatus::Unknown(
            "no public balance API — check z.ai/manage-account/usage".to_string(),
        ),
        _ => BalanceStatus::Error(format!("unknown provider '{name}'")),
    };
    let elapsed = match status {
        BalanceStatus::Amount { .. } | BalanceStatus::Error(_) => Some(start.elapsed()),
        BalanceStatus::Unknown(_) | BalanceStatus::NoKey => None,
    };
    Balance {
        provider: name,
        status,
        elapsed,
    }
}

// --- DeepSeek ---

#[derive(Deserialize)]
struct DeepSeekBalance {
    #[serde(default)]
    balance_infos: Vec<DeepSeekBalanceInfo>,
}

#[derive(Deserialize)]
struct DeepSeekBalanceInfo {
    currency: String,
    /// All numeric fields ship as strings ("110.00") in the public response.
    total_balance: String,
    #[serde(default)]
    granted_balance: Option<String>,
    #[serde(default)]
    topped_up_balance: Option<String>,
}

async fn fetch_deepseek(api_key: &str) -> BalanceStatus {
    let client = crate::config::http_client();
    let resp = match client
        .get("https://api.deepseek.com/user/balance")
        .header("Authorization", format!("Bearer {api_key}"))
        .timeout(BALANCE_TIMEOUT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return BalanceStatus::Error(format_reqwest_error(&e)),
    };
    let status = resp.status();
    if !status.is_success() {
        return BalanceStatus::Error(format!("HTTP {}", status.as_u16()));
    }
    let body: DeepSeekBalance = match resp.json().await {
        Ok(b) => b,
        Err(e) => return BalanceStatus::Error(format!("malformed response: {e}")),
    };
    let Some(info) = body.balance_infos.into_iter().next() else {
        return BalanceStatus::Error("no balance_infos in response".to_string());
    };
    let total = parse_amount(&info.total_balance).unwrap_or(0.0);
    let granted = info.granted_balance.as_deref().and_then(parse_amount);
    let topped_up = info.topped_up_balance.as_deref().and_then(parse_amount);
    BalanceStatus::Amount {
        currency: info.currency,
        total,
        granted,
        topped_up,
    }
}

// --- Kimi (Moonshot) ---

#[derive(Deserialize)]
struct KimiBalance {
    #[serde(default)]
    data: Option<KimiBalanceData>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)]
struct KimiBalanceData {
    #[serde(default)]
    available_balance: Option<f64>,
    #[serde(default)]
    voucher_balance: Option<f64>,
    #[serde(default)]
    cash_balance: Option<f64>,
}

async fn fetch_kimi(api_key: &str) -> BalanceStatus {
    let client = crate::config::http_client();
    // Honor a user-supplied base URL so users on the China endpoint
    // (api.moonshot.cn, balance reported in CNY) don't have to fork the code.
    let base = crate::config::config_get("LLM_KIMI_BASE_URL")
        .unwrap_or_else(|| "https://api.moonshot.ai".to_string());
    let url = format!("{}/v1/users/me/balance", base.trim_end_matches('/'));
    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .timeout(BALANCE_TIMEOUT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return BalanceStatus::Error(format_reqwest_error(&e)),
    };
    let status = resp.status();
    if !status.is_success() {
        return BalanceStatus::Error(format!("HTTP {}", status.as_u16()));
    }
    let body: KimiBalance = match resp.json().await {
        Ok(b) => b,
        Err(e) => return BalanceStatus::Error(format!("malformed response: {e}")),
    };
    let Some(data) = body.data else {
        return BalanceStatus::Error("no data field in response".to_string());
    };
    let available = data.available_balance.unwrap_or(0.0);
    // Currency depends on the endpoint — global (.ai) returns USD, China
    // (.cn) returns CNY. Pick from the configured base URL.
    let currency = if base.contains(".cn") { "CNY" } else { "USD" };
    BalanceStatus::Amount {
        currency: currency.to_string(),
        total: available,
        granted: data.voucher_balance,
        topped_up: data.cash_balance,
    }
}

// --- Helpers ---

fn parse_amount(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

fn format_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".to_string()
    } else if e.is_connect() {
        "connect failed".to_string()
    } else {
        format!("error: {e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_providers_match_key_names() {
        for &(_, key_name) in BALANCE_PROVIDERS {
            assert!(
                keys::KEY_NAMES.contains(&key_name),
                "{key_name} missing from keys::KEY_NAMES"
            );
        }
    }

    #[test]
    fn balance_providers_match_cloud_model_catalog() {
        use std::collections::HashSet;
        // Every cloud provider in the model catalog should appear here so the
        // user sees the full list under `/balance`. Local providers (ollama,
        // gguf, mlx) are intentionally excluded — no balance to report.
        let cloud_only: HashSet<&str> = crate::llm::MODELS
            .iter()
            .map(|&(p, _, _)| p)
            .filter(|&p| !matches!(p, "ollama" | "gguf" | "mlx"))
            .collect();
        let probed: HashSet<&str> = BALANCE_PROVIDERS.iter().map(|&(p, _)| p).collect();
        assert_eq!(
            cloud_only, probed,
            "BALANCE_PROVIDERS must match the cloud providers in llm::MODELS"
        );
    }

    #[test]
    fn parse_amount_handles_string_floats() {
        assert!((parse_amount("110.00").unwrap() - 110.0).abs() < 1e-9);
        assert!((parse_amount(" 0.50 ").unwrap() - 0.5).abs() < 1e-9);
        assert!(parse_amount("not a number").is_none());
    }

    #[tokio::test]
    async fn fetch_one_returns_no_key_when_secret_missing() {
        let b = fetch_one("openai", None).await;
        assert!(matches!(b.status, BalanceStatus::NoKey));
        assert_eq!(b.provider, "openai");
        assert!(b.elapsed.is_none());
    }

    #[tokio::test]
    async fn fetch_one_unknown_provider_returns_error() {
        let b = fetch_one("nonexistent", Some("key")).await;
        assert!(matches!(b.status, BalanceStatus::Error(_)));
    }

    #[tokio::test]
    async fn unknown_providers_carry_dashboard_hint() {
        for name in ["anthropic", "openai", "gemini", "grok", "mistral", "zai"] {
            let b = fetch_one(name, Some("dummy-key")).await;
            match b.status {
                BalanceStatus::Unknown(hint) => {
                    assert!(
                        !hint.is_empty(),
                        "{name}: Unknown variant must carry a hint"
                    );
                }
                other => panic!("{name}: expected Unknown, got {other:?}"),
            }
        }
    }
}
