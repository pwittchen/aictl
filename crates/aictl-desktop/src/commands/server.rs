//! aictl-server connection commands.
//!
//! The desktop can route LLM calls through a self-hosted `aictl-server`
//! when `Provider::AictlServer` is selected. This pane lets the user
//! configure the host URL (`AICTL_CLIENT_HOST`) and prove the
//! connection works end-to-end (`/healthz` + `/v1/models` master-key
//! probe).

use aictl_core::config;
use serde::Serialize;

#[derive(Serialize)]
pub struct ServerStatus {
    pub host: Option<String>,
    pub master_key_set: bool,
    pub fully_configured: bool,
}

#[tauri::command]
pub fn server_status() -> ServerStatus {
    let host = config::config_get("AICTL_CLIENT_HOST")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let master_key_set = aictl_core::keys::get_secret("AICTL_CLIENT_MASTER_KEY")
        .filter(|s| !s.is_empty())
        .is_some();
    let fully_configured = config::active_server().is_some();
    ServerStatus {
        host,
        master_key_set,
        fully_configured,
    }
}

#[derive(Serialize)]
pub struct ProbeResult {
    pub healthz_ok: bool,
    pub healthz_status: Option<u16>,
    pub healthz_error: Option<String>,
    pub models_ok: bool,
    pub models_status: Option<u16>,
    pub models_error: Option<String>,
    pub model_count: Option<usize>,
}

/// Hit `${host}/healthz` (no auth) and `${host}/v1/models` (with the
/// master key) and report what came back. Mirrors the CLI's `/ping`
/// command shape so the user can confirm both reachability and key
/// validity in one click.
#[tauri::command]
pub async fn server_probe() -> Result<ProbeResult, String> {
    let Some((url, key)) = config::active_server() else {
        return Err(
            "aictl-server is not configured (set AICTL_CLIENT_HOST and the master key first)"
                .to_string(),
        );
    };
    let base = url.trim_end_matches('/').to_string();

    let client = config::http_client();

    let mut out = ProbeResult {
        healthz_ok: false,
        healthz_status: None,
        healthz_error: None,
        models_ok: false,
        models_status: None,
        models_error: None,
        model_count: None,
    };

    let timeout = std::time::Duration::from_secs(5);
    match client
        .get(format!("{base}/healthz"))
        .timeout(timeout)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            out.healthz_status = Some(status.as_u16());
            out.healthz_ok = status.is_success();
            if !status.is_success() {
                out.healthz_error =
                    Some(status.canonical_reason().unwrap_or("non-2xx").to_string());
            }
        }
        Err(e) => {
            out.healthz_error = Some(format!("{e}"));
        }
    }

    match client
        .get(format!("{base}/v1/models"))
        .bearer_auth(&key)
        .timeout(timeout)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            out.models_status = Some(status.as_u16());
            out.models_ok = status.is_success();
            if status.is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let n = body
                        .get("data")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len);
                    out.model_count = n;
                }
            } else {
                out.models_error = Some(status.canonical_reason().unwrap_or("non-2xx").to_string());
            }
        }
        Err(e) => {
            out.models_error = Some(format!("{e}"));
        }
    }

    Ok(out)
}

#[derive(Serialize)]
pub struct OllamaStatus {
    pub host: String,
    pub default_host: &'static str,
    pub overridden: bool,
}

const OLLAMA_DEFAULT_HOST: &str = "http://localhost:11434";

#[tauri::command]
pub fn ollama_status() -> OllamaStatus {
    let configured = config::config_get("LLM_OLLAMA_HOST")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    OllamaStatus {
        host: configured
            .clone()
            .unwrap_or_else(|| OLLAMA_DEFAULT_HOST.to_string()),
        default_host: OLLAMA_DEFAULT_HOST,
        overridden: configured.is_some(),
    }
}

#[derive(Serialize)]
pub struct OllamaProbeResult {
    pub ok: bool,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub model_count: Option<usize>,
    /// Up to 12 model names, sorted as Ollama returned them. The Models
    /// tab uses `llm::ollama::list_models` for the canonical picker;
    /// this is just a proof-of-life summary.
    pub sample_models: Vec<String>,
}

/// Hit `${LLM_OLLAMA_HOST}/api/tags` and report whether the daemon is
/// reachable. Mirrors the CLI's `/ping` for Ollama.
#[tauri::command]
pub async fn ollama_probe() -> Result<OllamaProbeResult, String> {
    let host = config::config_get("LLM_OLLAMA_HOST")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| OLLAMA_DEFAULT_HOST.to_string());
    let base = host.trim_end_matches('/').to_string();

    let client = config::http_client();
    let timeout = std::time::Duration::from_secs(5);

    let mut out = OllamaProbeResult {
        ok: false,
        status: None,
        error: None,
        model_count: None,
        sample_models: vec![],
    };

    match client
        .get(format!("{base}/api/tags"))
        .timeout(timeout)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            out.status = Some(status.as_u16());
            out.ok = status.is_success();
            if !status.is_success() {
                out.error = Some(status.canonical_reason().unwrap_or("non-2xx").to_string());
                return Ok(out);
            }
            match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    let arr = body
                        .get("models")
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    out.model_count = Some(arr.len());
                    out.sample_models = arr
                        .iter()
                        .filter_map(|m| {
                            m.get("name")
                                .and_then(serde_json::Value::as_str)
                                .map(String::from)
                        })
                        .take(12)
                        .collect();
                }
                Err(e) => {
                    out.error = Some(format!("body parse: {e}"));
                }
            }
        }
        Err(e) => {
            out.error = Some(format!("{e}"));
        }
    }

    Ok(out)
}
