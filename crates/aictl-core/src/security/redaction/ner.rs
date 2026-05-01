//! Optional Layer-C Named Entity Recognition detector for the
//! redaction layer, powered by [gline-rs](https://crates.io/crates/gline-rs)
//! (`GLiNER` ONNX models via the `ort` runtime). Detects person names,
//! locations, and organizations that no regex can catch reliably.
//!
//! # Split compilation
//!
//! - **Always compiled** — model storage layout, `list_models`,
//!   `remove_model`, `clear_models`, `download_model`, spec parsing,
//!   `is_available()`. Users can pull / manage NER models from any
//!   aictl build; only the actual inference path is feature-gated.
//! - **Behind `redaction-ner` feature** — loading `GLiNER`, running
//!   inference, and producing [`Match`](super::Match) entries.
//!
//! # Storage layout
//!
//! A pulled model lives under `~/.aictl/models/ner/<name>/` with two
//! required files matching gline-rs's expected layout:
//!
//! ```text
//! ~/.aictl/models/ner/<name>/tokenizer.json
//! ~/.aictl/models/ner/<name>/onnx/model.onnx
//! ```
//!
//! # Spec formats for `--pull-ner-model`
//!
//! - `owner/repo` (Hugging Face shorthand) — default shape, e.g.
//!   `onnx-community/gliner_small-v2.1`.
//! - `hf:owner/repo` — explicit HF prefix.
//!
//! The local name is derived from the repo segment unless overridden.
//!
//! # Runtime behavior
//!
//! When `AICTL_REDACTION_NER=true` and the feature is built in,
//! [`run_ner_detector`] lazily loads the configured model on first
//! use (cached process-wide) and runs span-mode inference over each
//! outbound message. Detected entities become [`DetectorKind`] matches
//! (`PersonName` / `Location` / `Organization`). If the model is
//! missing or fails to load, the call logs a one-time warning and
//! returns no matches — Layers A + B still run.

use std::path::PathBuf;

use crate::config::config_get_scoped;
use crate::error::AictlError;
use crate::ui::AgentUI;

/// Default gline-rs-compatible model when `AICTL_REDACTION_NER_MODEL`
/// is not set. Small enough (~200 MB) to pull on demand, accurate
/// enough for person/location/organization extraction.
pub const DEFAULT_NER_MODEL: &str = "onnx-community/gliner_small-v2.1";

/// Entity labels passed to `GLiNER` on every inference call. These match
/// the labels the model was fine-tuned on and produce typed
/// [`DetectorKind`](super::DetectorKind) entries when matched.
#[cfg(feature = "redaction-ner")]
const DEFAULT_ENTITY_LABELS: &[&str] = &["person", "location", "organization"];

/// Minimum probability threshold for the default `Parameters`. Keeps
/// noisy low-confidence hits out of the match list.
#[cfg(feature = "redaction-ner")]
const DEFAULT_THRESHOLD: f32 = 0.5;

/// Returns true when this build includes native NER inference support.
/// Management commands work regardless.
#[must_use]
pub fn is_available() -> bool {
    cfg!(feature = "redaction-ner")
}

/// `~/.aictl/models/ner/` — base directory for NER model subfolders.
#[must_use]
pub fn models_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/models/ner"))
}

fn ensure_models_dir() -> std::io::Result<PathBuf> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Return the model directory for `name`, if the directory exists.
#[must_use]
pub fn model_dir(name: &str) -> Option<PathBuf> {
    let dir = models_dir().join(name);
    if dir.is_dir() { Some(dir) } else { None }
}

/// Paths to the two files gline-rs expects, or `None` if either is
/// missing on disk. Used by `run_ner_detector` to fail closed.
#[must_use]
pub fn model_files(name: &str) -> Option<(PathBuf, PathBuf)> {
    let dir = model_dir(name)?;
    let tokenizer = dir.join("tokenizer.json");
    let onnx = dir.join("onnx").join("model.onnx");
    if tokenizer.is_file() && onnx.is_file() {
        Some((tokenizer, onnx))
    } else {
        None
    }
}

/// Resolve the active NER model name from config, falling back to
/// [`DEFAULT_NER_MODEL`]. Strips any owner prefix (`owner/repo` →
/// `repo`) to match the `list_models()` / `model_dir()` naming scheme.
///
/// Honors `AICTL_SERVER_REDACTION_NER_MODEL` when running inside the
/// server so the proxy can ship a different (e.g. larger) NER model
/// than the CLI without forking config.
#[must_use]
pub fn configured_model_name() -> String {
    let raw = config_get_scoped(
        "AICTL_SERVER_REDACTION_NER_MODEL",
        "AICTL_REDACTION_NER_MODEL",
    )
    .unwrap_or_else(|| DEFAULT_NER_MODEL.to_string());
    default_name_from_spec(&raw).unwrap_or(raw)
}

/// List every directory under `~/.aictl/models/ner/` that contains
/// both required gline-rs files. Returns only fully usable models so
/// the `/security` display and CLI listings never advertise a broken
/// download.
#[must_use]
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
            let name = path.file_name()?.to_str()?.to_string();
            if model_files(&name).is_some() {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// Remove a pulled NER model by name (recursively deletes the
/// `~/.aictl/models/ner/<name>/` directory).
///
/// # Errors
///
/// Returns an I/O error if the directory cannot be removed.
pub fn remove_model(name: &str) -> std::io::Result<()> {
    validate_name(name).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let dir = models_dir().join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
        #[cfg(feature = "redaction-ner")]
        invalidate_cached_model(name);
    }
    Ok(())
}

/// Remove every pulled NER model. Returns the number of directories
/// deleted.
///
/// # Errors
///
/// Returns an I/O error on any failure to read or remove entries.
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
    #[cfg(feature = "redaction-ner")]
    invalidate_all_cached_models();
    Ok(count)
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "invalid local model name '{name}' (allowed: alphanumerics, '-', '_', '.')"
        ));
    }
    Ok(())
}

/// Parse a model spec into `(owner, repo, default_local_name)`.
///
/// Accepted forms:
/// * `hf:owner/repo` — explicit Hugging Face prefix
/// * `owner/repo`   — shorthand
///
/// # Errors
///
/// Returns a descriptive error when the spec does not match either form.
pub fn parse_spec(spec: &str) -> Result<(String, String, String), String> {
    let body = spec.strip_prefix("hf:").unwrap_or(spec);
    let Some((owner, repo)) = body.split_once('/') else {
        return Err(format!(
            "invalid NER model spec '{spec}' (expected owner/repo or hf:owner/repo)"
        ));
    };
    if owner.is_empty() || repo.is_empty() {
        return Err(format!(
            "invalid NER model spec '{spec}' (empty owner or repo)"
        ));
    }
    let default_name = repo.replace('/', "_");
    Ok((owner.to_string(), repo.to_string(), default_name))
}

fn default_name_from_spec(spec: &str) -> Option<String> {
    parse_spec(spec).ok().map(|(_, _, name)| name)
}

/// Download `tokenizer.json` + `onnx/model.onnx` from Hugging Face
/// into `~/.aictl/models/ner/<name>/`. Overwrites any existing
/// directory with the same name. Returns the resolved local name.
///
/// # Errors
///
/// Returns an error if the spec is invalid, the name contains
/// disallowed characters, the HTTP request fails, or the archive
/// cannot be written to disk.
pub async fn download_model(
    ui: &dyn AgentUI,
    spec: &str,
    override_name: Option<&str>,
) -> Result<String, AictlError> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let (owner, repo, default_name) = parse_spec(spec)?;
    let name = override_name.map_or(default_name, std::string::ToString::to_string);
    validate_name(&name)?;

    let base_dir = ensure_models_dir()?;
    let final_dir = base_dir.join(&name);
    let tmp_dir = base_dir.join(format!("{name}.part"));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)?;
    }
    std::fs::create_dir_all(tmp_dir.join("onnx"))?;

    let client = crate::config::http_client();

    // Two files, fetched in sequence with a shared progress bar per
    // file. gline-rs doesn't care about the rest of the HF repo
    // (config.json, special_tokens_map.json, …), so we fetch only
    // what we need and keep downloads small.
    let files = [
        ("tokenizer.json", "tokenizer.json"),
        ("onnx/model.onnx", "onnx/model.onnx"),
    ];

    for (idx, (remote, local)) in files.iter().enumerate() {
        let url =
            format!("https://huggingface.co/{owner}/{repo}/resolve/main/{remote}?download=true");
        let response = client
            .get(&url)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| {
                AictlError::Other(format!("failed to fetch {remote} from {owner}/{repo}: {e}"))
            })?;
        let total = response.content_length().unwrap_or(0);

        let label = format!("[{}/{}] {remote}", idx + 1, files.len());
        let progress = ui.progress_begin(&label, (total > 0).then_some(total));

        let dest = tmp_dir.join(local);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut out = tokio::fs::File::create(&dest).await?;
        let mut stream = response.bytes_stream();
        let mut got: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            out.write_all(&chunk).await?;
            got = got.saturating_add(chunk.len() as u64);
            ui.progress_update(&progress, got, None);
        }
        out.flush().await?;
        drop(out);
        ui.progress_end(progress, None);
    }

    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir)?;
    }
    std::fs::rename(&tmp_dir, &final_dir)?;

    #[cfg(feature = "redaction-ner")]
    invalidate_cached_model(&name);

    Ok(name)
}

// --- Inference (feature-gated) ---

#[cfg(feature = "redaction-ner")]
mod inference {
    use super::{DEFAULT_ENTITY_LABELS, DEFAULT_THRESHOLD, configured_model_name, model_files};
    use crate::security::redaction::{DetectorKind, Match, RedactionPolicy};
    use gliner::model::pipeline::span::SpanMode;
    use gliner::model::{GLiNER, input::text::TextInput, params::Parameters};
    use orp::params::RuntimeParameters;
    use std::sync::{Mutex, OnceLock};

    /// Per-process cache entry keyed by the loaded model's local name.
    /// Loading a `GLiNER` model takes ~1 second and pulls an ONNX session
    /// into memory — amortize across every redaction call.
    struct CachedModel {
        name: String,
        model: GLiNER<SpanMode>,
    }

    // Stored as Mutex<Option<Box<_>>> so invalidation on remove/clear
    // can drop the session without needing a separate writer lock.
    static CACHED: OnceLock<Mutex<Option<Box<CachedModel>>>> = OnceLock::new();

    /// One-shot warn flag — we don't want to spam the terminal every
    /// LLM turn if the user enabled NER but forgot to pull a model.
    static WARNED_MISSING: OnceLock<()> = OnceLock::new();
    static WARNED_LOAD_FAILURE: OnceLock<()> = OnceLock::new();

    fn cache() -> &'static Mutex<Option<Box<CachedModel>>> {
        CACHED.get_or_init(|| Mutex::new(None))
    }

    pub(crate) fn invalidate_cached_model(name: &str) {
        let Ok(mut slot) = cache().lock() else {
            return;
        };
        if slot.as_ref().is_some_and(|c| c.name == name) {
            *slot = None;
        }
    }

    pub(crate) fn invalidate_all_cached_models() {
        if let Ok(mut slot) = cache().lock() {
            *slot = None;
        }
    }

    fn load_or_warn(name: &str) -> Option<Box<CachedModel>> {
        let Some((tokenizer_path, onnx_path)) = model_files(name) else {
            if WARNED_MISSING.set(()).is_ok() {
                crate::ui::warn_global(&format!(
                    "redaction NER is enabled (AICTL_REDACTION_NER=true) but model '{name}' is not on disk. \
                     Run `aictl --pull-ner-model <owner>/<repo>` (e.g. onnx-community/gliner_small-v2.1) to fetch it, \
                     or unset AICTL_REDACTION_NER. Skipping NER this session."
                ));
            }
            return None;
        };

        match GLiNER::<SpanMode>::new(
            Parameters::default().with_threshold(DEFAULT_THRESHOLD),
            RuntimeParameters::default(),
            tokenizer_path,
            onnx_path,
        ) {
            Ok(model) => Some(Box::new(CachedModel {
                name: name.to_string(),
                model,
            })),
            Err(e) => {
                if WARNED_LOAD_FAILURE.set(()).is_ok() {
                    crate::ui::warn_global(&format!(
                        "failed to load NER model '{name}': {e}. Skipping NER this session."
                    ));
                }
                None
            }
        }
    }

    pub(crate) fn run_ner_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
        // Respect the per-kind enabled-detectors filter. If the user
        // explicitly listed only some detectors and none of ours are
        // in the list, skip loading the model entirely.
        let any_ner_enabled = pol.is_detector_enabled(&DetectorKind::PersonName)
            || pol.is_detector_enabled(&DetectorKind::Location)
            || pol.is_detector_enabled(&DetectorKind::Organization);
        if !any_ner_enabled {
            return;
        }

        let name = configured_model_name();
        let Ok(mut slot) = cache().lock() else {
            return;
        };
        if slot.as_ref().is_none_or(|c| c.name != name) {
            *slot = load_or_warn(&name);
        }
        let Some(cached) = slot.as_ref() else {
            return;
        };

        let Ok(input) = TextInput::from_str(&[text], DEFAULT_ENTITY_LABELS) else {
            // Malformed input (e.g. empty label list, but we control
            // that). Bail quietly — Layers A+B already ran.
            return;
        };
        let Ok(output) = cached.model.inference(input) else {
            // Inference failed — likely out-of-memory or a bad batch.
            // Don't warn per-call; just skip.
            return;
        };

        for spans in &output.spans {
            for span in spans {
                let (start, end) = span.offsets();
                if end <= start || end > text.len() {
                    continue;
                }
                // Safety: spans carry byte offsets into the input &str
                // (see `gliner::model::pipeline::context::EntityContext::create_span`).
                if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    continue;
                }
                let Some(kind) = classify(span.class()) else {
                    continue;
                };
                if !pol.is_detector_enabled(&kind) {
                    continue;
                }
                out.push(Match {
                    kind,
                    range: start..end,
                    confidence: confidence_label(span.probability()),
                });
            }
        }
    }

    fn classify(class: &str) -> Option<DetectorKind> {
        match class.to_ascii_lowercase().as_str() {
            "person" => Some(DetectorKind::PersonName),
            "location" => Some(DetectorKind::Location),
            "organization" | "org" => Some(DetectorKind::Organization),
            _ => None,
        }
    }

    fn confidence_label(p: f32) -> &'static str {
        if p >= 0.85 {
            "high"
        } else if p >= 0.65 {
            "medium"
        } else {
            "low"
        }
    }
}

#[cfg(feature = "redaction-ner")]
pub(crate) use inference::run_ner_detector;

#[cfg(feature = "redaction-ner")]
fn invalidate_cached_model(name: &str) {
    inference::invalidate_cached_model(name);
}

#[cfg(feature = "redaction-ner")]
fn invalidate_all_cached_models() {
    inference::invalidate_all_cached_models();
}

/// No-op stub so `find_matches` can call through unconditionally.
/// When the feature is off, the redactor just skips Layer C.
#[cfg(not(feature = "redaction-ner"))]
pub(crate) fn run_ner_detector(
    _text: &str,
    _pol: &crate::security::redaction::RedactionPolicy,
    _out: &mut Vec<crate::security::redaction::Match>,
) {
    // Feature not built in. If the user opted into NER anyway, we
    // already printed a warning at startup in `load_policy()`.
}

/// Runtime state for the `/security` display. Never exposes the actual
/// model handle — only enough to describe what's going on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NerStatus {
    /// Feature not compiled in.
    FeatureMissing,
    /// Feature built in but the user did not opt in.
    Disabled,
    /// Feature built + user opted in, but no model directory on disk.
    ModelMissing { expected_name: String },
    /// Feature built + user opted in + model present and usable.
    Ready { model_name: String },
}

/// Summarize the current NER backend state for `/security` and
/// startup-time diagnostics. Purely read-only.
#[must_use]
pub fn status(opted_in: bool) -> NerStatus {
    if !is_available() {
        return NerStatus::FeatureMissing;
    }
    if !opted_in {
        return NerStatus::Disabled;
    }
    let name = configured_model_name();
    if model_files(&name).is_some() {
        NerStatus::Ready { model_name: name }
    } else {
        NerStatus::ModelMissing {
            expected_name: name,
        }
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spec_shorthand() {
        let (o, r, n) = parse_spec("onnx-community/gliner_small-v2.1").unwrap();
        assert_eq!(o, "onnx-community");
        assert_eq!(r, "gliner_small-v2.1");
        assert_eq!(n, "gliner_small-v2.1");
    }

    #[test]
    fn parse_spec_hf_prefix() {
        let (o, r, n) = parse_spec("hf:onnx-community/gliner_small-v2.1").unwrap();
        assert_eq!(o, "onnx-community");
        assert_eq!(r, "gliner_small-v2.1");
        assert_eq!(n, "gliner_small-v2.1");
    }

    #[test]
    fn parse_spec_rejects_missing_slash() {
        assert!(parse_spec("justRepoName").is_err());
    }

    #[test]
    fn parse_spec_rejects_empty_parts() {
        assert!(parse_spec("/").is_err());
        assert!(parse_spec("owner/").is_err());
        assert!(parse_spec("/repo").is_err());
    }

    #[test]
    fn validate_name_accepts_common_shapes() {
        assert!(validate_name("gliner_small-v2.1").is_ok());
        assert!(validate_name("my-model.v1").is_ok());
        assert!(validate_name("abc").is_ok());
    }

    #[test]
    fn validate_name_rejects_bad_shapes() {
        assert!(validate_name("").is_err());
        assert!(validate_name("../escape").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a b").is_err());
    }

    #[test]
    fn default_model_constant_is_stable_hf_ref() {
        // The default must be a valid `owner/repo` spec so that
        // `configured_model_name()` can derive a sane local name.
        let (o, r, n) = parse_spec(DEFAULT_NER_MODEL).unwrap();
        assert!(!o.is_empty());
        assert!(!r.is_empty());
        assert!(!n.is_empty());
    }

    #[test]
    fn is_available_matches_cfg_flag() {
        assert_eq!(is_available(), cfg!(feature = "redaction-ner"));
    }

    #[test]
    fn status_reports_feature_flag_when_absent() {
        // status(true) returns FeatureMissing iff the feature is off.
        match status(true) {
            NerStatus::FeatureMissing => assert!(!cfg!(feature = "redaction-ner")),
            NerStatus::ModelMissing { .. } | NerStatus::Ready { .. } => {
                assert!(cfg!(feature = "redaction-ner"));
            }
            NerStatus::Disabled => panic!("status(true) must not return Disabled"),
        }
    }

    #[test]
    fn status_disabled_when_not_opted_in() {
        match status(false) {
            NerStatus::FeatureMissing => assert!(!cfg!(feature = "redaction-ner")),
            NerStatus::Disabled => assert!(cfg!(feature = "redaction-ner")),
            other => panic!("unexpected status: {other:?}"),
        }
    }
}
