//! Native local-model provider for Apple MLX models (macOS / Apple Silicon).
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
//! This module currently implements model management only. The actual
//! inference call is a stub; wiring in `mlx-rs` + safetensors + tokenizer is
//! tracked as a follow-up.
//!
//! Spec forms accepted by `download_model`:
//! * `mlx:owner/repo` — Hugging Face MLX repo (e.g. `mlx-community/...`)
//! * `owner/repo`     — shorthand for Hugging Face

use std::path::PathBuf;

use crate::Message;
use crate::llm::TokenUsage;

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
pub async fn download_model(
    spec: &str,
    override_name: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
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
        return Err(format!(
            "invalid local model name '{name}' (allowed: alphanumerics, '-', '_', '.')"
        )
        .into());
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
        .ok_or_else(|| -> Box<dyn std::error::Error> {
            format!("unexpected tree response for {owner}/{repo}").into()
        })?;

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
        return Err(format!("no downloadable files found in {owner}/{repo}").into());
    }

    // Ensure at least one safetensors file is present — otherwise the repo
    // probably isn't an MLX model and we'd produce a useless directory.
    if !files.iter().any(|f| f.ends_with(".safetensors")) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "repo {owner}/{repo} contains no .safetensors files — not an MLX model"
        )
        .into());
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

/// Extra reinforcement appended to the prompt for MLX-served local models.
/// Mirrors the GGUF path's reinforcement — same reasoning: small quantized
/// models reliably describe tools in prose instead of emitting the `<tool>`
/// XML the agent loop expects, and ending the prompt with an explicit format
/// contract + concrete few-shot example dramatically improves adherence.
#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
const MLX_TOOL_REINFORCEMENT: &str = r#"IMPORTANT: When you need to call a tool, the VERY FIRST thing in your response MUST be an XML tag in exactly this form:

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

// --- Inference (feature-gated, Apple Silicon only) ---
//
// The inference path is intentionally kept inside this file so the feature
// flag stays a single opt-in. It is split into small private modules below:
//
// * `arch`   — HF-naming Llama-family transformer built on `mlx-rs` primitives.
//              Works for Llama 3.x, Qwen 2.5, Mistral 7B v0.3, and DeepSeek-R1
//              Distill Qwen. Does NOT handle Gemma (logit softcap / sliding
//              window), Phi (different MLP), or MoE models.
// * `weights` — safetensors loader that walks `model.safetensors` or
//              `model.safetensors.index.json`, rewrites `q_proj.weight` →
//              `q_proj.inner.weight` so mlx-rs's `QuantizedLinear` param
//              paths match what mlx-community repos ship, and updates the
//              model in place.
// * `tmpl`   — chat-template renderer. Prefers the jinja template embedded
//              in `tokenizer_config.json` (rendered via `minijinja`);
//              falls back to a ChatML-like format if the template is
//              missing or fails to render.
// * `gen`    — top-level generation loop with KV cache + sampling.
//
// Known limitations in this first landing:
//   * Llama 3.1/3.2 RoPE scaling is NOT applied — we feed `rope_theta`
//     straight into `nn::Rope`. Short-context generation is fine; quality
//     past ~8K context will degrade.
//   * No streaming output — the generation loop returns the full string
//     when done. The REPL spinner stays active throughout.
//   * Models whose config reports an `architectures` value other than
//     `LlamaForCausalLM`, `Qwen2ForCausalLM`, `MistralForCausalLM`, or
//     `Qwen2MoeForCausalLM` (treated as Llama-like) are rejected.

#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
mod arch {
    use mlx_rs::{
        Array,
        builder::Builder,
        error::Exception,
        fast::{ScaledDotProductAttentionMask, scaled_dot_product_attention},
        macros::{ModuleParameters, Quantizable},
        module::Module,
        nn,
        ops::concatenate_axis,
        quantization::MaybeQuantized,
    };

    #[derive(Debug, Clone, serde::Deserialize)]
    pub struct LlamaConfig {
        pub hidden_size: i32,
        pub intermediate_size: i32,
        pub num_attention_heads: i32,
        #[serde(default)]
        pub num_key_value_heads: Option<i32>,
        pub num_hidden_layers: i32,
        #[serde(default = "default_rms_eps")]
        pub rms_norm_eps: f32,
        #[serde(default = "default_rope_theta")]
        pub rope_theta: f32,
        pub vocab_size: i32,
        #[serde(default)]
        pub head_dim: Option<i32>,
        #[serde(default)]
        pub tie_word_embeddings: bool,
        #[serde(default)]
        pub quantization: Option<QuantConfig>,
    }

    #[derive(Debug, Clone, serde::Deserialize)]
    pub struct QuantConfig {
        #[serde(default = "default_group_size")]
        pub group_size: i32,
        #[serde(default = "default_bits")]
        pub bits: i32,
    }

    fn default_rms_eps() -> f32 {
        1e-5
    }
    fn default_rope_theta() -> f32 {
        10_000.0
    }
    fn default_group_size() -> i32 {
        64
    }
    fn default_bits() -> i32 {
        4
    }

    impl LlamaConfig {
        pub fn head_dim(&self) -> i32 {
            self.head_dim
                .unwrap_or(self.hidden_size / self.num_attention_heads)
        }
        pub fn kv_heads(&self) -> i32 {
            self.num_key_value_heads.unwrap_or(self.num_attention_heads)
        }
    }

    #[derive(Debug, Clone, ModuleParameters, Quantizable)]
    pub struct Attention {
        n_heads: i32,
        n_kv_heads: i32,
        head_dim: i32,
        scale: f32,

        #[quantizable]
        #[param]
        pub q_proj: MaybeQuantized<nn::Linear>,

        #[quantizable]
        #[param]
        pub k_proj: MaybeQuantized<nn::Linear>,

        #[quantizable]
        #[param]
        pub v_proj: MaybeQuantized<nn::Linear>,

        #[quantizable]
        #[param]
        pub o_proj: MaybeQuantized<nn::Linear>,

        #[param]
        pub rope: nn::Rope,
    }

    pub struct AttnIn<'a> {
        pub x: &'a Array,
        pub mask: Option<ScaledDotProductAttentionMask<'a>>,
        pub cache: Option<(&'a Array, &'a Array)>,
    }
    pub struct AttnOut {
        pub output: Array,
        pub cache: (Array, Array),
    }

    impl Attention {
        pub fn new(cfg: &LlamaConfig) -> Result<Self, Exception> {
            let n_heads = cfg.num_attention_heads;
            let n_kv_heads = cfg.kv_heads();
            let head_dim = cfg.head_dim();
            let scale = (head_dim as f32).powf(-0.5);

            let q_proj = nn::LinearBuilder::new(cfg.hidden_size, n_heads * head_dim)
                .bias(false)
                .build()?;
            let k_proj = nn::LinearBuilder::new(cfg.hidden_size, n_kv_heads * head_dim)
                .bias(false)
                .build()?;
            let v_proj = nn::LinearBuilder::new(cfg.hidden_size, n_kv_heads * head_dim)
                .bias(false)
                .build()?;
            let o_proj = nn::LinearBuilder::new(n_heads * head_dim, cfg.hidden_size)
                .bias(false)
                .build()?;
            let rope = nn::RopeBuilder::new(head_dim).base(cfg.rope_theta).build()?;

            Ok(Self {
                n_heads,
                n_kv_heads,
                head_dim,
                scale,
                q_proj: MaybeQuantized::new(q_proj),
                k_proj: MaybeQuantized::new(k_proj),
                v_proj: MaybeQuantized::new(v_proj),
                o_proj: MaybeQuantized::new(o_proj),
                rope,
            })
        }
    }

    impl Module<AttnIn<'_>> for Attention {
        type Output = AttnOut;
        type Error = Exception;

        fn forward(&mut self, input: AttnIn<'_>) -> Result<Self::Output, Self::Error> {
            let AttnIn { x, mask, cache } = input;
            let b = x.shape()[0];
            let l = x.shape()[1];

            let mut q = self.q_proj.forward(x)?;
            let mut k = self.k_proj.forward(x)?;
            let mut v = self.v_proj.forward(x)?;

            q = q
                .reshape(&[b, l, self.n_heads, self.head_dim])?
                .transpose_axes(&[0, 2, 1, 3])?;
            k = k
                .reshape(&[b, l, self.n_kv_heads, self.head_dim])?
                .transpose_axes(&[0, 2, 1, 3])?;
            v = v
                .reshape(&[b, l, self.n_kv_heads, self.head_dim])?
                .transpose_axes(&[0, 2, 1, 3])?;

            match cache {
                Some((kc, vc)) => {
                    let offset = kc.shape()[2];
                    q = self.rope.forward((&q, offset))?;
                    k = self.rope.forward((&k, offset))?;
                    k = concatenate_axis(&[kc, &k], 2)?;
                    v = concatenate_axis(&[vc, &v], 2)?;
                }
                None => {
                    q = self.rope.forward(&q)?;
                    k = self.rope.forward(&k)?;
                }
            }

            let out = scaled_dot_product_attention(q, &k, &v, self.scale, mask)?;
            let out = out.transpose_axes(&[0, 2, 1, 3])?.reshape(&[b, l, -1])?;
            let out = self.o_proj.forward(&out)?;

            Ok(AttnOut {
                output: out,
                cache: (k, v),
            })
        }

        fn training_mode(&mut self, mode: bool) {
            self.q_proj.training_mode(mode);
            self.k_proj.training_mode(mode);
            self.v_proj.training_mode(mode);
            self.o_proj.training_mode(mode);
        }
    }

    #[derive(Debug, Clone, ModuleParameters, Quantizable)]
    pub struct Mlp {
        #[quantizable]
        #[param]
        pub gate_proj: MaybeQuantized<nn::Linear>,
        #[quantizable]
        #[param]
        pub up_proj: MaybeQuantized<nn::Linear>,
        #[quantizable]
        #[param]
        pub down_proj: MaybeQuantized<nn::Linear>,
    }

    impl Mlp {
        pub fn new(cfg: &LlamaConfig) -> Result<Self, Exception> {
            let gate_proj = nn::LinearBuilder::new(cfg.hidden_size, cfg.intermediate_size)
                .bias(false)
                .build()?;
            let up_proj = nn::LinearBuilder::new(cfg.hidden_size, cfg.intermediate_size)
                .bias(false)
                .build()?;
            let down_proj = nn::LinearBuilder::new(cfg.intermediate_size, cfg.hidden_size)
                .bias(false)
                .build()?;
            Ok(Self {
                gate_proj: MaybeQuantized::new(gate_proj),
                up_proj: MaybeQuantized::new(up_proj),
                down_proj: MaybeQuantized::new(down_proj),
            })
        }
    }

    impl Module<&Array> for Mlp {
        type Output = Array;
        type Error = Exception;

        fn forward(&mut self, x: &Array) -> Result<Self::Output, Self::Error> {
            let gated = nn::silu(self.gate_proj.forward(x)?)?.multiply(self.up_proj.forward(x)?)?;
            self.down_proj.forward(&gated)
        }

        fn training_mode(&mut self, mode: bool) {
            self.gate_proj.training_mode(mode);
            self.up_proj.training_mode(mode);
            self.down_proj.training_mode(mode);
        }
    }

    #[derive(Debug, Clone, ModuleParameters, Quantizable)]
    pub struct Layer {
        #[quantizable]
        #[param]
        pub self_attn: Attention,
        #[quantizable]
        #[param]
        pub mlp: Mlp,
        #[param]
        pub input_layernorm: nn::RmsNorm,
        #[param]
        pub post_attention_layernorm: nn::RmsNorm,
    }

    impl Layer {
        pub fn new(cfg: &LlamaConfig) -> Result<Self, Exception> {
            Ok(Self {
                self_attn: Attention::new(cfg)?,
                mlp: Mlp::new(cfg)?,
                input_layernorm: nn::RmsNormBuilder::new(cfg.hidden_size)
                    .eps(cfg.rms_norm_eps)
                    .build()?,
                post_attention_layernorm: nn::RmsNormBuilder::new(cfg.hidden_size)
                    .eps(cfg.rms_norm_eps)
                    .build()?,
            })
        }
    }

    impl Module<AttnIn<'_>> for Layer {
        type Output = AttnOut;
        type Error = Exception;

        fn forward(&mut self, input: AttnIn<'_>) -> Result<Self::Output, Self::Error> {
            let AttnIn { x, mask, cache } = input;
            let h_norm = self.input_layernorm.forward(x)?;
            let attn = self.self_attn.forward(AttnIn {
                x: &h_norm,
                mask,
                cache,
            })?;
            let h = x.add(&attn.output)?;
            let r = self
                .mlp
                .forward(&self.post_attention_layernorm.forward(&h)?)?;
            let output = h.add(&r)?;
            Ok(AttnOut {
                output,
                cache: attn.cache,
            })
        }

        fn training_mode(&mut self, mode: bool) {
            self.self_attn.training_mode(mode);
            self.mlp.training_mode(mode);
            self.input_layernorm.training_mode(mode);
            self.post_attention_layernorm.training_mode(mode);
        }
    }

    #[derive(Debug, Clone, ModuleParameters, Quantizable)]
    pub struct Backbone {
        #[quantizable]
        #[param]
        pub embed_tokens: MaybeQuantized<nn::Embedding>,
        #[quantizable]
        #[param]
        pub layers: Vec<Layer>,
        #[param]
        pub norm: nn::RmsNorm,
    }

    #[derive(Debug, Clone, ModuleParameters, Quantizable)]
    pub struct LlamaModel {
        #[quantizable]
        #[param]
        pub model: Backbone,
        /// Only present when `tie_word_embeddings=false` — otherwise we reuse
        /// `model.embed_tokens` for the output projection.
        #[param]
        pub lm_head: Option<MaybeQuantized<nn::Linear>>,

        pub tied: bool,
    }

    pub struct LlamaInput<'a> {
        pub inputs: &'a Array,
        pub cache: &'a [Option<(Array, Array)>],
    }
    pub struct LlamaOutput {
        pub logits: Array,
        pub cache: Vec<Option<(Array, Array)>>,
    }

    impl LlamaModel {
        pub fn new(cfg: &LlamaConfig) -> Result<Self, Exception> {
            let embed_tokens = nn::Embedding::new(cfg.vocab_size, cfg.hidden_size)?;
            let layers = (0..cfg.num_hidden_layers)
                .map(|_| Layer::new(cfg))
                .collect::<Result<Vec<_>, _>>()?;
            let norm = nn::RmsNormBuilder::new(cfg.hidden_size)
                .eps(cfg.rms_norm_eps)
                .build()?;
            let lm_head = if cfg.tie_word_embeddings {
                None
            } else {
                Some(MaybeQuantized::new(
                    nn::LinearBuilder::new(cfg.hidden_size, cfg.vocab_size)
                        .bias(false)
                        .build()?,
                ))
            };
            Ok(Self {
                model: Backbone {
                    embed_tokens: MaybeQuantized::new(embed_tokens),
                    layers,
                    norm,
                },
                lm_head,
                tied: cfg.tie_word_embeddings,
            })
        }

        pub fn forward_full(&mut self, input: LlamaInput<'_>) -> Result<LlamaOutput, Exception> {
            let LlamaInput { inputs, cache } = input;
            let mut h = self.model.embed_tokens.forward(inputs)?;

            let mut mask = None;
            if h.shape()[1] > 1 {
                let m =
                    nn::MultiHeadAttention::create_additive_causal_mask::<f32>(h.shape()[1])?;
                let m = m.as_dtype(h.dtype())?;
                mask = Some(m);
            }

            let mut out_cache = Vec::with_capacity(self.model.layers.len());
            for (i, layer) in self.model.layers.iter_mut().enumerate() {
                let entry = cache.get(i).and_then(Option::as_ref).map(|(k, v)| (k, v));
                let out = layer.forward(AttnIn {
                    x: &h,
                    mask: mask.as_ref().map(Into::into),
                    cache: entry,
                })?;
                h = out.output;
                out_cache.push(Some(out.cache));
            }

            let h = self.model.norm.forward(&h)?;
            let logits = match &mut self.lm_head {
                Some(head) => head.forward(&h)?,
                None => {
                    // Tied embeddings: logits = h @ embed_tokens.weight^T.
                    // Both nn::Embedding and nn::QuantizedEmbedding expose
                    // an `as_linear` helper that does exactly this — the
                    // quantized variant uses the on-device quantized_matmul
                    // kernel so we don't need to dequantize.
                    match &self.model.embed_tokens {
                        MaybeQuantized::Original(e) => e.as_linear(&h)?,
                        MaybeQuantized::Quantized(q) => q.as_linear(&h)?,
                    }
                }
            };

            Ok(LlamaOutput {
                logits,
                cache: out_cache,
            })
        }
    }
}

#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
mod weights {
    use mlx_rs::Array;
    use mlx_rs::module::{ModuleParameters, ModuleParametersExt, Param};
    use mlx_rs::nn::{Embedding, QuantizedEmbedding};
    use mlx_rs::quantization::MaybeQuantized;
    // Needed to call parameters_mut/freeze across the parameter tree.
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::arch::LlamaModel;

    /// Resolve the safetensors file(s) in a model directory. Supports both
    /// `model.safetensors` (single file) and sharded models that carry a
    /// `model.safetensors.index.json`.
    pub fn shard_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let single = dir.join("model.safetensors");
        if single.exists() {
            return Ok(vec![single]);
        }
        let index = dir.join("model.safetensors.index.json");
        if !index.exists() {
            return Err(format!(
                "no safetensors files in {} (expected model.safetensors or model.safetensors.index.json)",
                dir.display()
            ));
        }
        let body = std::fs::read_to_string(&index)
            .map_err(|e| format!("failed to read {}: {e}", index.display()))?;
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| format!("failed to parse {}: {e}", index.display()))?;
        let map = parsed
            .get("weight_map")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("{} missing weight_map", index.display()))?;
        let mut files: Vec<String> = map
            .values()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        files.sort();
        Ok(files.into_iter().map(|f| dir.join(f)).collect())
    }

    /// Rewrite mlx-community safetensors keys so they match mlx-rs's
    /// `QuantizedLinear` parameter paths. mlx-community ships
    /// `...q_proj.weight` (packed u32), `...q_proj.scales`, `...q_proj.biases`,
    /// but the Rust `QuantizedLinear` struct nests the weight inside an
    /// `inner` Linear, so the expected path is `...q_proj.inner.weight`.
    /// Scales/biases live directly on `QuantizedLinear` and don't need
    /// rewriting.
    ///
    /// Embeddings have the same quirk: `model.embed_tokens.weight` stays as
    /// the top-level Embedding's weight in both quantized and non-quantized
    /// builds, so no rewrite is needed for it. The rewrite only applies to
    /// linear layers whose parent struct wraps them in `MaybeQuantized`.
    pub fn translate_quantized_key(key: &str) -> String {
        // Only rewrite `<path>.weight` when `<path>` ends in a name we know
        // corresponds to a MaybeQuantized<Linear> field. Everything else
        // passes through unchanged so scales/biases and RmsNorm params land
        // where they should.
        // Field names whose `MaybeQuantized<...>` wrapper, when quantized,
        // hides the underlying weight under `.inner.weight`. Linear projections
        // and the token embedding both use this layout in mlx-rs.
        const LINEAR_FIELDS: &[&str] = &[
            "q_proj",
            "k_proj",
            "v_proj",
            "o_proj",
            "gate_proj",
            "up_proj",
            "down_proj",
            "lm_head",
            "embed_tokens",
        ];
        if let Some(stripped) = key.strip_suffix(".weight") {
            for field in LINEAR_FIELDS {
                if stripped.ends_with(field) {
                    return format!("{stripped}.inner.weight");
                }
            }
        }
        key.to_string()
    }

    /// Manually install the quantized token-embedding weights. The
    /// `ModuleParameters` derive on our `Backbone` doesn't expose the
    /// `MaybeQuantized<Embedding>` field in its flattened parameter tree
    /// (linears with the same wrapper *do* show up — apparently a quirk of
    /// the macro), so the safetensors entries `model.embed_tokens.weight`,
    /// `model.embed_tokens.scales`, `model.embed_tokens.biases` would
    /// otherwise be silently unused. We pull them out of the merged
    /// HashMap and construct a `QuantizedEmbedding` in place.
    fn install_quantized_embedding(
        model: &mut LlamaModel,
        merged: &mut std::collections::HashMap<String, Array>,
        group_size: i32,
        bits: i32,
    ) -> Result<(), String> {
        // After my key translation, the linear-style embedding weight
        // lives at `model.embed_tokens.inner.weight`. scales/biases pass
        // through unchanged.
        let weight = merged
            .remove("model.embed_tokens.inner.weight")
            .ok_or_else(|| "missing model.embed_tokens.inner.weight in safetensors".to_string())?;
        let scales = merged
            .remove("model.embed_tokens.scales")
            .ok_or_else(|| "missing model.embed_tokens.scales in safetensors".to_string())?;
        let biases = merged
            .remove("model.embed_tokens.biases")
            .ok_or_else(|| "missing model.embed_tokens.biases in safetensors".to_string())?;

        let qe = QuantizedEmbedding {
            group_size,
            bits,
            scales: Param::new(scales),
            biases: Param::new(biases),
            inner: Embedding {
                weight: Param::new(weight),
            },
        };
        model.model.embed_tokens = MaybeQuantized::Quantized(qe);
        Ok(())
    }

    /// Load every shard into a merged HashMap of {translated_key → Array},
    /// then update the model's parameters in place.
    pub fn load_model_weights(
        model: &mut LlamaModel,
        dir: &Path,
        quantized: bool,
        group_size: i32,
        bits: i32,
    ) -> Result<(), String> {
        let shards = shard_paths(dir)?;
        let mut merged: HashMap<String, Array> = HashMap::new();

        for shard in &shards {
            let loaded = Array::load_safetensors(shard)
                .map_err(|e| format!("failed to load {}: {e}", shard.display()))?;
            for (k, v) in loaded {
                let key = if quantized {
                    translate_quantized_key(&k)
                } else {
                    k
                };
                merged.insert(key, v);
            }
        }

        // QuantizedLinear and QuantizedEmbedding call `freeze_parameters(true)`
        // on themselves during construction. Unfreeze everything before
        // updating so every param is assignable.
        model.unfreeze_parameters(true);

        // Hand-install the quantized token embedding before the regular
        // load loop runs so its safetensors entries don't end up reported
        // as "unused". See `install_quantized_embedding` for the reason
        // the embedding can't go through the normal parameter tree.
        if quantized {
            install_quantized_embedding(model, &mut merged, group_size, bits)?;
        }

        let mut params = model.parameters_mut().flatten();
        let mut missing: Vec<String> = Vec::new();
        let keys: Vec<String> = params.keys().map(std::string::ToString::to_string).collect();
        for key in &keys {
            match merged.remove(key) {
                Some(arr) => {
                    if let Some(p) = params.get_mut(&**key) {
                        **p = arr;
                    }
                }
                None => missing.push(key.clone()),
            }
        }

        // Anything left in `merged` is a file key that didn't land on any
        // model parameter. That's almost always what causes gibberish
        // generation — some layers silently keep their random-init weights
        // while the "real" weight just sits unused in the HashMap.
        let leftover: Vec<String> = merged.keys().cloned().collect();

        if !missing.is_empty() || !leftover.is_empty() {
            let missing_sample = missing
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let leftover_sample = leftover
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            // Show the first few model param paths that touch `embed_tokens`
            // so we can see what the model actually expects vs what the file
            // ships. Falls back to the first 8 keys overall if nothing
            // matches the embedding prefix.
            let mut embed_paths: Vec<&String> = keys
                .iter()
                .filter(|k| k.contains("embed"))
                .take(8)
                .collect();
            if embed_paths.is_empty() {
                embed_paths = keys.iter().take(8).collect();
            }
            let embed_sample = embed_paths
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "weight-loading mismatch: {} model param(s) unfilled, {} file tensor(s) unused (out of {} total model params, {} total file tensors).\n  unfilled model params (first 8): {}\n  unused file tensors (first 8): {}\n  model param paths near 'embed' (or first 8): {}",
                missing.len(),
                leftover.len(),
                keys.len(),
                keys.len() - missing.len() + leftover.len(),
                if missing_sample.is_empty() {
                    "(none)".to_string()
                } else {
                    missing_sample
                },
                if leftover_sample.is_empty() {
                    "(none)".to_string()
                } else {
                    leftover_sample
                },
                if embed_sample.is_empty() {
                    "(none)".to_string()
                } else {
                    embed_sample
                },
            ));
        }

        // Parameters updated — evaluate so the lazy graph materializes.
        model
            .eval()
            .map_err(|e| format!("parameter eval failed: {e}"))?;
        Ok(())
    }
}

#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
mod tmpl {
    use crate::{Message, Role};
    use minijinja::value::Value;

    /// Render a chat template loaded from tokenizer_config.json into a
    /// ready-to-tokenize string. Returns `Ok(text)` on success; `Err(reason)`
    /// so the caller can fall back to a generic ChatML envelope.
    pub fn render(template: &str, messages: &[Message]) -> Result<String, String> {
        let mut env = minijinja::Environment::new();
        env.add_template("chat", template)
            .map_err(|e| format!("chat template compile failed: {e}"))?;
        let tmpl = env.get_template("chat").map_err(|e| e.to_string())?;
        let msgs: Vec<Value> = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                Value::from_serialize(&serde_json::json!({
                    "role": role,
                    "content": m.content,
                }))
            })
            .collect();
        tmpl.render(minijinja::context! {
            messages => Value::from(msgs),
            add_generation_prompt => true,
            bos_token => "",
            eos_token => "",
        })
        .map_err(|e| format!("chat template render failed: {e}"))
    }

    /// ChatML-ish fallback used when the repo's template can't be rendered.
    /// Not canonical for any specific model family, but produces coherent
    /// output for most instruction-tuned Llama/Qwen/Mistral checkpoints.
    pub fn chatml_fallback(messages: &[Message], reinforcement: &str) -> String {
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
        out.push_str("<|im_start|>system\n");
        out.push_str(reinforcement);
        out.push_str("<|im_end|>\n");
        out.push_str("<|im_start|>assistant\n");
        out
    }
}

#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
#[allow(clippy::too_many_lines)]
pub async fn call_mlx(
    model: &str,
    messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    use crate::Role;
    use mlx_rs::Array;
    use mlx_rs::ops::indexing::{IndexOp, NewAxis};
    use mlx_rs::transforms::eval;
    use std::collections::HashSet;
    use tokenizers::Tokenizer;

    let dir = model_path(model).ok_or_else(|| -> Box<dyn std::error::Error> {
        format!(
            "MLX model '{model}' not found. Pull it with `aictl --pull-mlx-model <spec>` or via `/mlx` in the REPL."
        )
        .into()
    })?;

    // --- Load config.json ---
    let cfg_path = dir.join("config.json");
    let cfg_body = std::fs::read_to_string(&cfg_path)
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("failed to read {}: {e}", cfg_path.display()).into()
        })?;
    let cfg_raw: serde_json::Value = serde_json::from_str(&cfg_body)?;

    if let Some(arches) = cfg_raw.get("architectures").and_then(|v| v.as_array()) {
        let names: Vec<&str> = arches.iter().filter_map(|v| v.as_str()).collect();
        let supported = names.iter().any(|n| {
            matches!(
                *n,
                "LlamaForCausalLM"
                    | "Qwen2ForCausalLM"
                    | "MistralForCausalLM"
                    | "Qwen2MoeForCausalLM"
            )
        });
        if !supported && !names.is_empty() {
            return Err(format!(
                "unsupported model architecture: {} — only Llama-family models are supported in this build",
                names.join(", ")
            )
            .into());
        }
    }

    let cfg: arch::LlamaConfig = serde_json::from_str(&cfg_body)?;

    // --- EOS tokens (scalar or list) ---
    let mut eos_ids: HashSet<u32> = HashSet::new();
    match cfg_raw.get("eos_token_id") {
        Some(serde_json::Value::Number(n)) => {
            if let Some(id) = n.as_u64() {
                eos_ids.insert(id as u32);
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            for v in arr {
                if let Some(id) = v.as_u64() {
                    eos_ids.insert(id as u32);
                }
            }
        }
        _ => {}
    }

    // --- Tokenizer + chat template ---
    let tok_path = dir.join("tokenizer.json");
    let tokenizer = Tokenizer::from_file(&tok_path)
        .map_err(|e| -> Box<dyn std::error::Error> { format!("tokenizer load: {e}").into() })?;

    let tok_cfg_path = dir.join("tokenizer_config.json");
    let tok_cfg: serde_json::Value = if tok_cfg_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&tok_cfg_path)?)?
    } else {
        serde_json::Value::Null
    };

    // Also pick up EOS ids hinted by tokenizer_config (some repos list them here only).
    if let Some(tok) = tok_cfg.get("eos_token").and_then(|v| v.as_str())
        && let Some(enc) = tokenizer.encode(tok, false).ok()
        && let [id, ..] = enc.get_ids()
    {
        eos_ids.insert(*id);
    }

    let reinforcement = MLX_TOOL_REINFORCEMENT;
    let mut messages_with_tail: Vec<Message> = messages.to_vec();
    messages_with_tail.push(Message {
        role: Role::System,
        content: reinforcement.to_string(),
        images: vec![],
    });

    let prompt_text = match tok_cfg.get("chat_template").and_then(|v| v.as_str()) {
        Some(template) => match tmpl::render(template, &messages_with_tail) {
            Ok(s) => s,
            Err(_) => tmpl::chatml_fallback(messages, reinforcement),
        },
        None => tmpl::chatml_fallback(messages, reinforcement),
    };

    // --- Heavy work: build model, load weights, run generation ---
    let cfg_for_spawn = cfg.clone();
    let dir_for_spawn = dir.clone();
    let prompt_for_spawn = prompt_text.clone();
    let eos_for_spawn: Vec<u32> = eos_ids.into_iter().collect();

    let result = tokio::task::spawn_blocking(
        move || -> Result<(String, u64, u64), String> {
            // Build model, optionally convert to quantized variant.
            let mut mdl = arch::LlamaModel::new(&cfg_for_spawn)
                .map_err(|e| format!("failed to build model: {e}"))?;
            let quantized = cfg_for_spawn.quantization.is_some();
            if let Some(q) = cfg_for_spawn.quantization.as_ref() {
                mdl = mlx_rs::nn::quantize(mdl, Some(q.group_size), Some(q.bits))
                    .map_err(|e| format!("quantize wrap failed: {e}"))?;
            }

            // Load weights from shard(s).
            let (group_size, bits) = cfg_for_spawn
                .quantization
                .as_ref()
                .map_or((64, 4), |q| (q.group_size, q.bits));
            weights::load_model_weights(&mut mdl, &dir_for_spawn, quantized, group_size, bits)?;

            // Tokenize the prompt.
            let enc = tokenizer
                .encode(&prompt_for_spawn[..], true)
                .map_err(|e| format!("tokenize failed: {e}"))?;
            let prompt_ids = enc.get_ids();
            let input_tokens = prompt_ids.len() as u64;
            if prompt_ids.is_empty() {
                return Err("empty prompt after tokenization".into());
            }

            let prompt_arr = Array::from(prompt_ids).index(NewAxis);

            // Prefill.
            let initial_cache: Vec<Option<(Array, Array)>> = Vec::new();
            let out = mdl
                .forward_full(arch::LlamaInput {
                    inputs: &prompt_arr,
                    cache: &initial_cache,
                })
                .map_err(|e| format!("prefill failed: {e}"))?;
            let mut cache = out.cache;
            let mut next = sample(&out.logits.index((.., -1, ..))).map_err(|e| e.to_string())?;

            let eos_set: HashSet<u32> = eos_for_spawn.into_iter().collect();
            let mut generated: Vec<u32> = Vec::with_capacity(4096);

            const MAX_NEW: usize = 4096;
            for _ in 0..MAX_NEW {
                eval(std::iter::once(&next)).map_err(|e| format!("eval failed: {e}"))?;
                let id: u32 = next.item::<u32>();
                if eos_set.contains(&id) {
                    break;
                }
                generated.push(id);

                // Short-circuit on end-of-turn / common stop markers that
                // lots of chat templates emit as plain text instead of a
                // dedicated EOS id.
                if let Ok(partial) = tokenizer.decode(&generated, true)
                    && (partial.contains("<|im_end|>")
                        || partial.contains("<|eot_id|>")
                        || partial.contains("</s>"))
                {
                    break;
                }

                let tok_arr = next.index((.., NewAxis));
                let step = mdl
                    .forward_full(arch::LlamaInput {
                        inputs: &tok_arr,
                        cache: cache.as_slice(),
                    })
                    .map_err(|e| format!("decode step failed: {e}"))?;
                cache = step.cache;
                let logits = step
                    .logits
                    .squeeze_axes(&[1])
                    .map_err(|e| e.to_string())?;
                next = sample(&logits).map_err(|e| e.to_string())?;
            }

            let output_tokens = generated.len() as u64;
            let mut text = tokenizer
                .decode(&generated, true)
                .map_err(|e| format!("decode failed: {e}"))?;

            for marker in ["<|im_end|>", "<|eot_id|>", "</s>"] {
                if let Some(idx) = text.find(marker) {
                    text.truncate(idx);
                }
            }

            Ok((text.trim().to_string(), input_tokens, output_tokens))
        },
    )
    .await
    .map_err(|e| -> Box<dyn std::error::Error> {
        format!("inference task panicked: {e}").into()
    })?;

    let (text, input_tokens, output_tokens) =
        result.map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    Ok((
        text,
        TokenUsage {
            input_tokens,
            output_tokens,
            ..TokenUsage::default()
        },
    ))
}

/// Sampler used by `call_mlx`. Temperature 0.2 (match the GGUF path's
/// preset) produces clean, parseable tool calls on small quantized models.
/// Top-p truncation is deliberately omitted in this first cut — categorical
/// sampling from temperature-scaled logits is good enough to ship, and
/// mlx-rs does not expose a ready-made top-p helper. Tracked as a quality
/// follow-up.
#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
fn sample(logits: &mlx_rs::Array) -> Result<mlx_rs::Array, mlx_rs::error::Exception> {
    use mlx_rs::array;
    use mlx_rs::random::categorical;

    const TEMP: f32 = 0.2;
    let scaled = logits.multiply(array!(1.0 / TEMP))?;
    categorical(&scaled, None, None, None)
}

#[cfg(not(all(feature = "mlx", target_os = "macos", target_arch = "aarch64")))]
#[allow(clippy::unused_async)]
pub async fn call_mlx(
    _model: &str,
    _messages: &[Message],
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    if !host_supports_mlx() {
        return Err("MLX inference is only available on macOS + Apple Silicon (aarch64).".into());
    }
    Err("MLX inference is not compiled in. Rebuild with `cargo build --features mlx` on macOS Apple Silicon.".into())
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
