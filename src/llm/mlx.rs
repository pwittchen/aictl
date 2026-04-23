//! Native MLX model provider for Apple Silicon (macOS / aarch64).
//!
//! Models are multi-file Hugging Face repos (safetensors + tokenizer config +
//! chat template metadata) stored under `~/.aictl/models/mlx/<name>/`. They
//! are downloaded on demand via `/mlx` in the REPL or `--pull-mlx-model <spec>`
//! on the CLI; nothing is bundled into the binary.
//!
//! Inference is gated behind the `mlx` cargo feature, which is only meant to be
//! enabled on `macos` + `aarch64`. When the feature is off (or the host is not
//! Apple Silicon) the download/list/remove commands still work — they just
//! produce models that cannot yet be run, and `call_mlx` returns a clear error
//! telling the user how to enable native inference.
//!
//! Spec forms accepted by `download_model`:
//! * `mlx:owner/repo` — Hugging Face MLX repo (e.g. `mlx-community/...`)
//! * `owner/repo`     — shorthand for Hugging Face

use std::path::PathBuf;

use crate::error::AictlError;

mod arch;
mod gemma2;
mod inference;
mod tmpl;
mod weights;

pub use inference::call_mlx;

/// Return true when this build includes native MLX inference support.
/// Only intended to be true on macOS + aarch64.
pub fn is_available() -> bool {
    cfg!(all(
        feature = "mlx",
        target_os = "macos",
        target_arch = "aarch64"
    ))
}

/// Return true when the current host can in principle run MLX (Apple Silicon).
/// Used to decide whether to surface friendly "this build can't run MLX"
/// messages vs. "this platform can't run MLX at all" messages.
pub fn host_supports_mlx() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// Directory where local MLX model directories live.
pub fn models_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/models/mlx"))
}

fn ensure_models_dir() -> std::io::Result<PathBuf> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// List the names of locally downloaded MLX models (each a subdirectory of
/// the MLX models dir that contains a `config.json`). Returns an empty vec
/// if the dir does not exist or is empty.
pub fn list_models() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(models_dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if !path.is_dir() {
                return None;
            }
            // Require a config.json so half-downloaded or unrelated dirs don't show up.
            if !path.join("config.json").exists() {
                return None;
            }
            path.file_name()
                .and_then(|s| s.to_str())
                .map(std::string::ToString::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Resolve a model name to its on-disk directory. Returns None if not downloaded.
#[cfg_attr(not(feature = "mlx"), allow(dead_code))]
pub fn model_path(name: &str) -> Option<PathBuf> {
    let path = models_dir().join(name);
    if path.is_dir() && path.join("config.json").exists() {
        Some(path)
    } else {
        None
    }
}

/// Remove a downloaded model directory by name.
pub fn remove_model(name: &str) -> std::io::Result<()> {
    let path = models_dir().join(name);
    if !path.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("MLX model '{name}' not found"),
        ));
    }
    // Refuse to remove anything outside the models dir (defence in depth
    // against a pathological name containing separators).
    let canonical = path.canonicalize()?;
    let root = models_dir().canonicalize()?;
    if !canonical.starts_with(&root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to remove a path outside the MLX models directory",
        ));
    }
    std::fs::remove_dir_all(path)
}

/// Clear every downloaded MLX model.
pub fn clear_models() -> std::io::Result<usize> {
    let dir = models_dir();
    if !dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
            count += 1;
        }
    }
    Ok(count)
}

/// Compute the total on-disk size (in bytes) of a downloaded MLX model.
/// Returns 0 when the directory is missing or unreadable. Used by the `/mlx`
/// menu to display per-model size.
pub fn model_size(name: &str) -> u64 {
    let Some(path) = model_path(name) else {
        return 0;
    };
    walk_size(&path)
}

fn walk_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let p = entry.path();
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                total += meta.len();
            } else if meta.is_dir() {
                total += walk_size(&p);
            }
        }
    }
    total
}

/// Parse a model spec into (`owner`, `repo`, `local_name`).
///
/// Accepted forms:
/// * `mlx:owner/repo`
/// * `owner/repo`
fn parse_spec(spec: &str) -> Result<(String, String, String), String> {
    let body = spec.strip_prefix("mlx:").unwrap_or(spec);
    let Some((owner, repo)) = body.split_once('/') else {
        return Err(format!(
            "invalid mlx spec '{spec}' (expected 'mlx:owner/repo' or 'owner/repo')"
        ));
    };
    if owner.is_empty() || repo.contains('/') || repo.is_empty() {
        return Err(format!(
            "invalid mlx spec '{spec}' (expected exactly one '/' separating owner and repo)"
        ));
    }
    let name = format!("{owner}__{repo}");
    Ok((owner.to_string(), repo.to_string(), name))
}

/// Files in a Hugging Face repo that are irrelevant for inference and
/// shouldn't be downloaded. The list is conservative — anything not matched
/// here (safetensors, tokenizer*, config.json, `chat_template`*, etc.) is
/// pulled. Keeping this tight avoids multi-GB transfers of README images,
/// demo videos, duplicate `PyTorch` weights, etc.
fn should_skip_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    // Git plumbing
    if lower.starts_with(".git") || lower == ".gitattributes" {
        return true;
    }
    // Docs / media
    for ext in [
        ".md", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".mp4", ".mov", ".pdf",
    ] {
        if lower.ends_with(ext) {
            return true;
        }
    }
    // Non-MLX weight formats that some repos ship alongside MLX weights.
    // MLX repos always use safetensors, so .bin / .pt / .pth / .gguf / .onnx
    // can be skipped to keep the download small.
    for ext in [".bin", ".pt", ".pth", ".gguf", ".onnx", ".ot"] {
        if lower.ends_with(ext) {
            return true;
        }
    }
    false
}

/// Download all relevant files of an MLX model repo to
/// `~/.aictl/models/mlx/<name>/`. Prints a progress bar per file via
/// `indicatif`. Overwrites any existing directory with the same name.
/// Returns the resolved local name.
#[allow(clippy::too_many_lines)]
pub async fn download_model(spec: &str, override_name: Option<&str>) -> Result<String, AictlError> {
    use futures_util::StreamExt;
    use indicatif::{ProgressBar, ProgressStyle};
    use tokio::io::AsyncWriteExt;

    let (owner, repo, default_name) = parse_spec(spec)?;
    let name = override_name.map_or(default_name, std::string::ToString::to_string);
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(AictlError::Other(format!(
            "invalid local model name '{name}' (allowed: alphanumerics, '-', '_', '.')"
        )));
    }

    let dir = ensure_models_dir()?.join(&name);
    let tmp_dir = ensure_models_dir()?.join(format!("{name}.part"));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)?;
    }
    std::fs::create_dir_all(&tmp_dir)?;

    let client = crate::config::http_client();

    // List the repo tree.
    let tree_url =
        format!("https://huggingface.co/api/models/{owner}/{repo}/tree/main?recursive=1");
    let tree: serde_json::Value = client
        .get(&tree_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let entries = tree
        .as_array()
        .ok_or_else(|| AictlError::Other(format!("unexpected tree response for {owner}/{repo}")))?;

    let files: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            let t = e.get("type").and_then(|v| v.as_str())?;
            if t != "file" {
                return None;
            }
            let path = e.get("path").and_then(|v| v.as_str())?;
            if should_skip_file(path) {
                return None;
            }
            Some(path.to_string())
        })
        .collect();

    if files.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(AictlError::Other(format!(
            "no downloadable files found in {owner}/{repo}"
        )));
    }

    // Ensure at least one safetensors file is present — otherwise the repo
    // probably isn't an MLX model and we'd produce a useless directory.
    if !files.iter().any(|f| f.ends_with(".safetensors")) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(AictlError::Other(format!(
            "repo {owner}/{repo} contains no .safetensors files — not an MLX model"
        )));
    }

    for (idx, file) in files.iter().enumerate() {
        let url =
            format!("https://huggingface.co/{owner}/{repo}/resolve/main/{file}?download=true");
        let response = client.get(&url).send().await?.error_for_status()?;
        let total = response.content_length().unwrap_or(0);

        let pb = if total > 0 {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::with_template(
                    "  {spinner:.green} {msg} {bytes}/{total_bytes} ({bytes_per_sec}) {bar:30.cyan/blue}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            pb
        } else {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::with_template("  {spinner:.green} {msg} {bytes} ({bytes_per_sec})")
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            pb
        };
        pb.set_message(format!("[{}/{}] {file}", idx + 1, files.len()));

        let dest = tmp_dir.join(file);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut out = tokio::fs::File::create(&dest).await?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            out.write_all(&chunk).await?;
            pb.inc(chunk.len() as u64);
        }
        out.flush().await?;
        drop(out);
        pb.finish_and_clear();
    }

    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    tokio::fs::rename(&tmp_dir, &dir).await?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mlx_prefix() {
        let (owner, repo, name) =
            parse_spec("mlx:mlx-community/Llama-3.2-3B-Instruct-4bit").unwrap();
        assert_eq!(owner, "mlx-community");
        assert_eq!(repo, "Llama-3.2-3B-Instruct-4bit");
        assert_eq!(name, "mlx-community__Llama-3.2-3B-Instruct-4bit");
    }

    #[test]
    fn parse_shorthand() {
        let (owner, repo, name) = parse_spec("mlx-community/Qwen2.5-7B-Instruct-4bit").unwrap();
        assert_eq!(owner, "mlx-community");
        assert_eq!(repo, "Qwen2.5-7B-Instruct-4bit");
        assert_eq!(name, "mlx-community__Qwen2.5-7B-Instruct-4bit");
    }

    #[test]
    fn parse_invalid() {
        assert!(parse_spec("not-a-spec").is_err());
        assert!(parse_spec("mlx:only-owner").is_err());
        assert!(parse_spec("owner/repo/extra").is_err());
    }

    #[test]
    fn skip_filter() {
        assert!(should_skip_file("README.md"));
        assert!(should_skip_file("assets/preview.png"));
        assert!(should_skip_file("pytorch_model.bin"));
        assert!(should_skip_file(".gitattributes"));
        assert!(!should_skip_file("model.safetensors"));
        assert!(!should_skip_file("tokenizer.json"));
        assert!(!should_skip_file("config.json"));
        assert!(!should_skip_file("tokenizer_config.json"));
    }
}
