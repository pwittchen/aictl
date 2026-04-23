//! Native GGUF model provider.
//!
//! Models are GGUF files stored in `~/.aictl/models/gguf/`. They are downloaded
//! on demand via `/gguf` in the REPL or `--pull-gguf-model <spec>` on the CLI;
//! nothing is bundled into the binary. When no model has been downloaded
//! the provider exposes no entries, so by default native models are
//! unavailable until the user explicitly pulls one.
//!
//! Inference itself is gated behind the `gguf` cargo feature which pulls
//! in `llama-cpp-2`. When that feature is disabled the download/list/remove
//! commands still work — they just produce models that can't yet be run,
//! and `call_gguf` returns a clear error telling the user to rebuild with
//! `--features gguf`.

use std::path::{Path, PathBuf};

use crate::error::AictlError;
use crate::llm::TokenUsage;
use crate::{Message, Role};

#[cfg(feature = "gguf")]
use std::sync::Arc;

/// Return true when this build includes native inference support.
pub fn is_available() -> bool {
    cfg!(feature = "gguf")
}

/// Directory where local GGUF models live.
pub fn models_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.aictl/models/gguf"))
}

fn ensure_models_dir() -> std::io::Result<PathBuf> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// List the names of locally downloaded models (the file stem of each
/// `*.gguf` file in the models dir). Returns an empty vec if the dir does
/// not exist or is empty.
pub fn list_models() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(models_dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("gguf") {
                return None;
            }
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(std::string::ToString::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Resolve a model name to its on-disk path. Returns None if not downloaded.
#[cfg_attr(not(feature = "gguf"), allow(dead_code))]
pub fn model_path(name: &str) -> Option<PathBuf> {
    let path = models_dir().join(format!("{name}.gguf"));
    if path.exists() { Some(path) } else { None }
}

/// Remove a downloaded model file by name.
pub fn remove_model(name: &str) -> std::io::Result<()> {
    let path = models_dir().join(format!("{name}.gguf"));
    let result = std::fs::remove_file(&path);
    #[cfg(feature = "gguf")]
    invalidate_cached_model(&path);
    result
}

/// Clear every downloaded model.
pub fn clear_models() -> std::io::Result<usize> {
    let dir = models_dir();
    if !dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("gguf") {
            std::fs::remove_file(&path)?;
            #[cfg(feature = "gguf")]
            invalidate_cached_model(&path);
            count += 1;
        }
    }
    Ok(count)
}

/// Parse a model spec into (`download_url`, `local_name`).
///
/// Accepted forms:
/// * `hf:owner/repo/path/to/file.gguf` — Hugging Face public file
/// * `https://…/file.gguf` — direct URL
/// * `owner/repo:filename.gguf` — shorthand for Hugging Face
fn parse_spec(spec: &str) -> Result<(String, String), String> {
    if let Some(rest) = spec.strip_prefix("hf:") {
        let parts: Vec<&str> = rest.splitn(3, '/').collect();
        if parts.len() < 3 {
            return Err(format!(
                "invalid hf spec '{spec}' (expected hf:owner/repo/filename.gguf)"
            ));
        }
        let owner = parts[0];
        let repo = parts[1];
        let file = parts[2];
        let url =
            format!("https://huggingface.co/{owner}/{repo}/resolve/main/{file}?download=true");
        let name = default_name_from_file(file);
        return Ok((url, name));
    }

    if spec.starts_with("http://") || spec.starts_with("https://") {
        let file = spec
            .rsplit('/')
            .next()
            .and_then(|f| f.split('?').next())
            .unwrap_or("model.gguf");
        return Ok((spec.to_string(), default_name_from_file(file)));
    }

    if let Some((repo, file)) = spec.split_once(':')
        && let Some((owner, repo_name)) = repo.split_once('/')
    {
        let url =
            format!("https://huggingface.co/{owner}/{repo_name}/resolve/main/{file}?download=true");
        return Ok((url, default_name_from_file(file)));
    }

    Err(format!(
        "invalid model spec '{spec}' (expected hf:owner/repo/file.gguf, owner/repo:file.gguf, or an https:// URL)"
    ))
}

/// Derive a user-friendly local name from a gguf filename by stripping the
/// extension and replacing path separators.
fn default_name_from_file(file: &str) -> String {
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file);
    stem.replace('/', "_")
}

/// Download a model to `~/.aictl/models/gguf/<name>.gguf`. Prints a progress bar
/// to stderr via `indicatif`. Overwrites any existing file with the same
/// name. Returns the resolved local name.
pub async fn download_model(spec: &str, override_name: Option<&str>) -> Result<String, AictlError> {
    use futures_util::StreamExt;
    use indicatif::{ProgressBar, ProgressStyle};
    use tokio::io::AsyncWriteExt;

    let (url, default_name) = parse_spec(spec)?;
    let name = override_name.map_or(default_name, std::string::ToString::to_string);
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "invalid local model name '{name}' (allowed: alphanumerics, '-', '_', '.')"
        )
        .into());
    }

    let dir = ensure_models_dir()?;
    let final_path = dir.join(format!("{name}.gguf"));
    let tmp_path = dir.join(format!("{name}.gguf.part"));

    let client = crate::config::http_client();
    let response = client.get(&url).send().await?.error_for_status()?;
    let total = response.content_length().unwrap_or(0);

    let pb = if total > 0 {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::with_template(
                "  {spinner:.green} {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta}) {bar:30.cyan/blue}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
        );
        pb
    } else {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("  {spinner:.green} {bytes} ({bytes_per_sec})")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        pb
    };

    let mut file = tokio::fs::File::create(&tmp_path).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }
    file.flush().await?;
    drop(file);
    pb.finish_and_clear();

    tokio::fs::rename(&tmp_path, &final_path).await?;
    Ok(name)
}

/// Extra reinforcement appended to the prompt for local models. Small
/// quantized models (especially 1–3B) reliably describe tools in prose
/// instead of emitting the `<tool>` XML the agent loop expects. Ending
/// the prompt with an explicit format contract and a concrete few-shot
/// example dramatically improves tool-call adherence — models mimic the
/// format they see most recently.
const LOCAL_TOOL_REINFORCEMENT: &str = r#"IMPORTANT: When you need to call a tool, the VERY FIRST thing in your response MUST be an XML tag in exactly this form:

<tool name="tool_name">
tool input here
</tool>

Do NOT describe how to use a tool in English. Do NOT write the tool name as plain text. Emit the literal <tool name="..."> XML tag and nothing else before it. If you can answer the user's question without a tool, reply in plain text with no <tool> tag at all.

Example of a correct tool call:
<tool name="fetch_datetime">
</tool>

Example of a correct tool call with input:
<tool name="read_file">
/path/to/file.txt
</tool>

Now respond to the user."#;

/// Flatten a `Message` list to a single prompt string using a generic
/// ChatML-like template. Most modern instruction-tuned GGUF models tolerate
/// this well enough to produce coherent output; specialized templates could
/// be added per model later. A final system turn reinforces the tool-call
/// XML contract — crucial for small local models that otherwise describe
/// tools in prose instead of invoking them.
#[cfg_attr(not(feature = "gguf"), allow(dead_code))]
fn render_prompt(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        out.push_str("<|im_start|>");
        out.push_str(role);
        out.push('\n');
        out.push_str(&m.content);
        out.push_str("<|im_end|>\n");
    }
    // Final system reminder — placed after the user's last turn so it's the
    // most recent instruction the model attends to.
    out.push_str("<|im_start|>system\n");
    out.push_str(LOCAL_TOOL_REINFORCEMENT);
    out.push_str("<|im_end|>\n");
    out.push_str("<|im_start|>assistant\n");
    out
}

#[cfg(feature = "gguf")]
fn silence_llama_logs() {
    use llama_cpp_2::{LogOptions, send_logs_to_tracing};
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // with_logs_enabled(false) installs the internal tracing callback
        // but makes it return early, so no text ever reaches stderr.
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    });
}

/// Return the process-wide `LlamaBackend`, initializing it once on first call.
///
/// `LlamaBackend::init()` can only succeed a single time per process — calling
/// it twice returns `BackendAlreadyInitialized`. Double-checked locking around
/// a `OnceLock` keeps the first init exclusive and hands out the same static
/// reference to every subsequent caller.
#[cfg(feature = "gguf")]
fn ensure_backend() -> Result<&'static llama_cpp_2::llama_backend::LlamaBackend, String> {
    use llama_cpp_2::llama_backend::LlamaBackend;
    use std::sync::{Mutex, OnceLock};

    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    static INIT_LOCK: Mutex<()> = Mutex::new(());

    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    let _guard = INIT_LOCK.lock().map_err(|e| e.to_string())?;
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    silence_llama_logs();
    let backend = LlamaBackend::init().map_err(|e| e.to_string())?;
    let _ = BACKEND.set(backend);
    Ok(BACKEND.get().expect("backend just initialized"))
}

/// Shared handle to the process-wide model cache. Both the loader and the
/// invalidation helper must reach the same `OnceLock` — keeping it in a
/// single accessor avoids accidentally creating two distinct statics.
#[cfg(feature = "gguf")]
fn model_cache() -> &'static std::sync::Mutex<
    std::collections::HashMap<PathBuf, Arc<llama_cpp_2::model::LlamaModel>>,
> {
    use llama_cpp_2::model::LlamaModel;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<LlamaModel>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return the cached `LlamaModel` for `path`, loading it from disk on first
/// access. Reloading weights on every turn costs tens of seconds for
/// quantized 7B+ models and made the agent loop look frozen — caching
/// amortizes the load across the session.
#[cfg(feature = "gguf")]
fn ensure_model(path: &Path) -> Result<Arc<llama_cpp_2::model::LlamaModel>, String> {
    use llama_cpp_2::model::LlamaModel;
    use llama_cpp_2::model::params::LlamaModelParams;

    let cache = model_cache();
    {
        let guard = cache.lock().map_err(|e| e.to_string())?;
        if let Some(model) = guard.get(path) {
            return Ok(model.clone());
        }
    }

    let backend = ensure_backend()?;
    let model_params = LlamaModelParams::default();
    let model = LlamaModel::load_from_file(backend, path, &model_params)
        .map_err(|e| format!("failed to load model: {e}"))?;
    let arc = Arc::new(model);

    let mut guard = cache.lock().map_err(|e| e.to_string())?;
    Ok(guard
        .entry(path.to_path_buf())
        .or_insert_with(|| arc.clone())
        .clone())
}

/// Drop any cached `LlamaModel` whose on-disk file has been removed. Called
/// from `remove_model` / `clear_models` so a later re-pull with the same name
/// doesn't reuse the stale mmap from the deleted file.
#[cfg(feature = "gguf")]
fn invalidate_cached_model(path: &Path) {
    if let Ok(mut guard) = model_cache().lock() {
        guard.remove(path);
    }
}

/// Known tool names. When a local model's prose output mentions any of these
/// without emitting a `<tool>` tag, we treat it as a near-miss and force-seed
/// the XML prefix so the model can complete the call.
#[cfg_attr(not(feature = "gguf"), allow(dead_code))]
const KNOWN_TOOL_NAMES: &[&str] = &[
    "exec_shell",
    "read_file",
    "write_file",
    "remove_file",
    "create_directory",
    "list_directory",
    "search_files",
    "edit_file",
    "find_files",
    "search_web",
    "fetch_url",
    "extract_website",
    "fetch_datetime",
    "fetch_geolocation",
    "read_image",
    "generate_image",
    "read_document",
];

/// Detect that the model produced tool-intent prose without an actual
/// `<tool>` tag. The caller has already verified the absence of `<tool`.
#[cfg_attr(not(feature = "gguf"), allow(dead_code))]
fn mentions_tool_use(text: &str) -> bool {
    let lower = text.to_lowercase();
    KNOWN_TOOL_NAMES.iter().any(|name| lower.contains(name))
}

#[cfg(feature = "gguf")]
fn token_piece(
    model: &llama_cpp_2::model::LlamaModel,
    token: llama_cpp_2::token::LlamaToken,
) -> String {
    use llama_cpp_2::TokenToStringError;
    let bytes = match model.token_to_piece_bytes(token, 32, false, None) {
        Ok(b) => b,
        Err(TokenToStringError::InsufficientBufferSpace(i)) => {
            let needed: usize = usize::try_from(-i).unwrap_or(1024);
            model
                .token_to_piece_bytes(token, needed, false, None)
                .unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(feature = "gguf")]
pub async fn call_gguf(
    model: &str,
    messages: &[Message],
    on_token: Option<crate::llm::TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::AddBos;
    use llama_cpp_2::sampling::LlamaSampler;
    use llama_cpp_2::token::data_array::LlamaTokenDataArray;
    use std::num::NonZeroU32;

    let path = model_path(model).ok_or_else(|| AictlError::Other(format!(
        "local model '{model}' not found. Pull it with `aictl --pull-gguf-model <spec>` or via `/gguf` in the REPL."
    )))?;

    // Load (or reuse) the backend and model outside the blocking task so the
    // expensive first-turn cost is paid once per session instead of per call.
    // `ensure_backend` touches llama.cpp's global init machinery, but the
    // actual work it guards is cheap after the first call; loading a model
    // from disk can take tens of seconds, so caching is what the user sees.
    let backend = ensure_backend()?;
    let model_arc = ensure_model(&path)?;

    let prompt = render_prompt(messages);

    // llama.cpp context state is not Send/Sync — keep context creation and
    // decoding inside a blocking task. The cached model and backend reference
    // are both `Send + Sync`, so we can move them in safely.
    let on_token = on_token.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(String, u64, u64), String> {
        let model = &*model_arc;

        let tokens = model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| format!("tokenization failed: {e}"))?;
        let input_tokens = tokens.len() as u64;

        // Batch must fit the whole prompt for the initial decode. We never
        // add more than one token per iteration after that, so headroom
        // beyond prompt length is unnecessary.
        let batch_capacity = tokens.len().max(512);
        // Context must be at least prompt + generation budget (4096 new tokens).
        // 4096 matches the upper bound used by the agent loop's tool-calling
        // flow on API providers, so local models don't get their `<tool>` tags
        // truncated mid-emission on long reasoning traces.
        let n_ctx = u32::try_from(batch_capacity + 4096)
            .unwrap_or(8192)
            .max(8192);

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(u32::try_from(batch_capacity).unwrap_or(512));
        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| format!("failed to create context: {e}"))?;

        let mut batch = LlamaBatch::new(batch_capacity, 1);
        let last_idx = (tokens.len() - 1) as i32;
        for (i, tok) in tokens.iter().enumerate() {
            let is_last = i as i32 == last_idx;
            batch
                .add(*tok, i as i32, &[0], is_last)
                .map_err(|e| e.to_string())?;
        }
        ctx.decode(&mut batch).map_err(|e| e.to_string())?;

        let mut n_cur = batch.n_tokens();
        let max_new: i32 = 4096;
        let mut out = String::new();
        let mut output_tokens: u64 = 0;
        let eos = model.token_eos();

        // Low-temperature sampler chain. Pure greedy sampling (previously used)
        // sometimes locks small quantized models into repetition loops and
        // produces malformed `<tool>` tags; temp 0.2 + min_p 0.05 + top_p 0.9
        // is the standard "structured output" preset and yields clean, parseable
        // tool calls far more reliably without hurting determinism much.
        let sampler = LlamaSampler::chain_simple([
            LlamaSampler::min_p(0.05, 1),
            LlamaSampler::top_p(0.9, 1),
            LlamaSampler::temp(0.2),
            LlamaSampler::dist(0),
        ]);

        for _ in 0..max_new {
            let mut candidates = LlamaTokenDataArray::from_iter(ctx.candidates(), false);
            candidates.apply_sampler(&sampler);
            let next = candidates
                .selected_token()
                .ok_or_else(|| "sampler failed to select a token".to_string())?;
            if next == eos {
                break;
            }
            let piece = token_piece(model, next);
            if piece.contains("<|im_end|>") {
                break;
            }
            if let Some(ref sink) = on_token {
                sink(&piece);
            }
            out.push_str(&piece);
            output_tokens += 1;

            batch.clear();
            batch
                .add(next, n_cur, &[0], true)
                .map_err(|e| e.to_string())?;
            n_cur += 1;
            ctx.decode(&mut batch).map_err(|e| e.to_string())?;
        }

        // Force-continuation: when the model clearly describes using a tool
        // in prose but fails to emit the `<tool>` XML tag, seed `\n<tool name="`
        // into the context and let the model complete the call. Small 1–3B
        // quantized models frequently hit this near-miss — they understand
        // which tool to use but can't switch from prose to structured output.
        // Once the XML prefix is already in the state, completing it becomes
        // a much easier task.
        if !out.contains("<tool") && mentions_tool_use(&out) {
            let forced_prefix = "\n<tool name=\"";
            let forced_tokens = model
                .str_to_token(forced_prefix, AddBos::Never)
                .map_err(|e| format!("forced-continuation tokenization failed: {e}"))?;
            let last = forced_tokens.len().saturating_sub(1);
            for (i, tok) in forced_tokens.iter().enumerate() {
                let is_last = i == last;
                batch.clear();
                batch
                    .add(*tok, n_cur, &[0], is_last)
                    .map_err(|e| e.to_string())?;
                n_cur += 1;
                ctx.decode(&mut batch).map_err(|e| e.to_string())?;
            }
            if let Some(ref sink) = on_token {
                sink(forced_prefix);
            }
            out.push_str(forced_prefix);

            for _ in 0..512 {
                let mut candidates = LlamaTokenDataArray::from_iter(ctx.candidates(), false);
                candidates.apply_sampler(&sampler);
                let next = candidates
                    .selected_token()
                    .ok_or_else(|| "sampler failed to select a token".to_string())?;
                if next == eos {
                    break;
                }
                let piece = token_piece(model, next);
                if piece.contains("<|im_end|>") {
                    break;
                }
                out.push_str(&piece);
                output_tokens += 1;

                batch.clear();
                batch
                    .add(next, n_cur, &[0], true)
                    .map_err(|e| e.to_string())?;
                n_cur += 1;
                ctx.decode(&mut batch).map_err(|e| e.to_string())?;

                if out.contains("</tool>") {
                    break;
                }
            }
        }

        drop(sampler);

        // Strip any trailing ChatML end-of-turn marker the model may have emitted.
        if let Some(idx) = out.find("<|im_end|>") {
            out.truncate(idx);
        }
        Ok((out.trim().to_string(), input_tokens, output_tokens))
    })
    .await
    .map_err(|e| AictlError::Other(format!("inference task failed: {e}")))?;

    let (text, input_tokens, output_tokens) = result.map_err(AictlError::Other)?;

    Ok((
        text,
        TokenUsage {
            input_tokens,
            output_tokens,
            ..TokenUsage::default()
        },
    ))
}

#[cfg(not(feature = "gguf"))]
#[allow(clippy::unused_async)]
pub async fn call_gguf(
    _model: &str,
    _messages: &[Message],
    _on_token: Option<crate::llm::TokenSink>,
) -> Result<(String, TokenUsage), AictlError> {
    Err(AictlError::Other(
        "native GGUF model inference is not compiled in. Rebuild with `cargo build --features gguf` (requires cmake and a C/C++ toolchain).".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hf_spec() {
        let (url, name) = parse_spec("hf:owner/repo/file.Q4_K_M.gguf").unwrap();
        assert!(url.contains("huggingface.co/owner/repo/resolve/main/file.Q4_K_M.gguf"));
        assert_eq!(name, "file.Q4_K_M");
    }

    #[test]
    fn parse_shorthand_spec() {
        let (url, name) = parse_spec("owner/repo:file.gguf").unwrap();
        assert!(url.contains("huggingface.co/owner/repo/resolve/main/file.gguf"));
        assert_eq!(name, "file");
    }

    #[test]
    fn parse_https_spec() {
        let (url, name) = parse_spec("https://example.com/path/model.gguf").unwrap();
        assert_eq!(url, "https://example.com/path/model.gguf");
        assert_eq!(name, "model");
    }

    #[test]
    fn parse_invalid_spec() {
        assert!(parse_spec("not-a-spec").is_err());
        assert!(parse_spec("hf:owner/repo").is_err());
    }

    #[test]
    fn default_name_strips_extension() {
        assert_eq!(default_name_from_file("llama-3.gguf"), "llama-3");
        assert_eq!(default_name_from_file("sub/dir/model.gguf"), "model");
    }

    #[test]
    fn render_prompt_includes_all_roles() {
        let msgs = vec![
            Message {
                role: Role::System,
                content: "sys".to_string(),
                images: vec![],
            },
            Message {
                role: Role::User,
                content: "hi".to_string(),
                images: vec![],
            },
        ];
        let p = render_prompt(&msgs);
        assert!(p.contains("<|im_start|>system\nsys<|im_end|>"));
        assert!(p.contains("<|im_start|>user\nhi<|im_end|>"));
        // Prompt now ends with a tool-use reinforcement system turn.
        assert!(p.contains(LOCAL_TOOL_REINFORCEMENT));
        assert!(p.ends_with("<|im_start|>assistant\n"));
    }
}
