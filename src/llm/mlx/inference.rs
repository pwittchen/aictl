//! Top-level generation loop with KV cache + sampling, split into
//! `run_llama_inference` and `run_gemma2_inference`. Also hosts the
//! `call_mlx` entry point that wires config parsing → tokenizer load →
//! chat-template rendering → `spawn_blocking` onto the right worker.
//!
//! Known limitations in this first landing:
//!   * Llama 3.1/3.2 `RoPE` scaling is NOT applied — we feed `rope_theta`
//!     straight into `nn::Rope`. Short-context generation is fine; quality
//!     past ~8K context will degrade.
//!   * No streaming output — the generation loop returns the full string
//!     when done. The REPL spinner stays active throughout.
//!   * Gemma 2 sliding-window layers keep the full KV cache instead of
//!     truncating to the last `sliding_window` tokens; correctness is
//!     preserved by a sliding-window mask, but long-context decoding
//!     wastes memory.
//!   * Models whose config reports an `architectures` value other than
//!     `LlamaForCausalLM`, `Qwen2ForCausalLM`, `MistralForCausalLM`,
//!     `Qwen2MoeForCausalLM`, or `Gemma2ForCausalLM` are rejected.

use crate::Message;
use crate::llm::TokenUsage;

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

/// Llama-family inference worker. Runs synchronously on a worker
/// thread (spawned by `call_mlx`). Builds the model, loads safetensors
/// weights, tokenizes the prompt, and runs the prefill + decode loop
/// until EOS or the hard `MAX_NEW` cap.
#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
fn run_llama_inference(
    cfg: super::arch::LlamaConfig,
    dir: std::path::PathBuf,
    prompt: String,
    eos: Vec<u32>,
    tokenizer: tokenizers::Tokenizer,
    on_token: Option<crate::llm::TokenSink>,
) -> Result<(String, u64, u64), String> {
    use mlx_rs::Array;
    use mlx_rs::ops::indexing::{IndexOp, NewAxis};
    use mlx_rs::transforms::eval;
    use std::collections::HashSet;

    use super::arch;
    use super::weights;

    let mut mdl = arch::LlamaModel::new(&cfg).map_err(|e| format!("failed to build model: {e}"))?;
    let quantized = cfg.quantization.is_some();
    if let Some(q) = cfg.quantization.as_ref() {
        mdl = mlx_rs::nn::quantize(mdl, Some(q.group_size), Some(q.bits))
            .map_err(|e| format!("quantize wrap failed: {e}"))?;
    }
    let (group_size, bits) = cfg
        .quantization
        .as_ref()
        .map_or((64, 4), |q| (q.group_size, q.bits));
    weights::load_model_weights(&mut mdl, &dir, quantized, group_size, bits)?;

    let enc = tokenizer
        .encode(&prompt[..], true)
        .map_err(|e| format!("tokenize failed: {e}"))?;
    let prompt_ids = enc.get_ids();
    let input_tokens = prompt_ids.len() as u64;
    if prompt_ids.is_empty() {
        return Err("empty prompt after tokenization".into());
    }
    let prompt_arr = Array::from(prompt_ids).index(NewAxis);

    let initial_cache: Vec<Option<(Array, Array)>> = Vec::new();
    let out = mdl
        .forward_full(arch::LlamaInput {
            inputs: &prompt_arr,
            cache: &initial_cache,
        })
        .map_err(|e| format!("prefill failed: {e}"))?;
    let mut cache = out.cache;
    let mut next = sample(&out.logits.index((.., -1, ..))).map_err(|e| e.to_string())?;

    let eos_set: HashSet<u32> = eos.into_iter().collect();
    let mut generated: Vec<u32> = Vec::with_capacity(4096);
    // Cumulative-decoded prefix already forwarded to the on_token callback.
    // BPE/SentencePiece tokenizers can't reliably decode a single token in
    // isolation (multi-byte UTF-8 / leading-space pieces depend on prior
    // context), so we decode the whole vec each step and emit the new suffix.
    let mut emitted_prefix = String::new();

    const MAX_NEW: usize = 4096;
    for _ in 0..MAX_NEW {
        eval(std::iter::once(&next)).map_err(|e| format!("eval failed: {e}"))?;
        let id: u32 = next.item::<u32>();
        if eos_set.contains(&id) {
            break;
        }
        generated.push(id);

        if let Ok(partial) = tokenizer.decode(&generated, true) {
            if partial.contains("<|im_end|>")
                || partial.contains("<|eot_id|>")
                || partial.contains("</s>")
            {
                break;
            }
            if let Some(ref sink) = on_token
                && partial.len() > emitted_prefix.len()
                && partial.starts_with(&emitted_prefix)
            {
                let delta = &partial[emitted_prefix.len()..];
                if !delta.is_empty() {
                    sink(delta);
                    emitted_prefix = partial;
                }
            }
        }

        let tok_arr = next.index((.., NewAxis));
        let step = mdl
            .forward_full(arch::LlamaInput {
                inputs: &tok_arr,
                cache: cache.as_slice(),
            })
            .map_err(|e| format!("decode step failed: {e}"))?;
        cache = step.cache;
        let logits = step.logits.squeeze_axes(&[1]).map_err(|e| e.to_string())?;
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
}

/// Gemma 2 inference worker. Mirrors `run_llama_inference` but
/// dispatches to `gemma2::Gemma2Model` and also watches for Gemma's
/// `<end_of_turn>` marker as a stop condition (some repos don't list
/// it in `eos_token_id`).
#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
fn run_gemma2_inference(
    cfg: super::gemma2::Gemma2Config,
    dir: std::path::PathBuf,
    prompt: String,
    eos: Vec<u32>,
    tokenizer: tokenizers::Tokenizer,
    on_token: Option<crate::llm::TokenSink>,
) -> Result<(String, u64, u64), String> {
    use mlx_rs::Array;
    use mlx_rs::ops::indexing::{IndexOp, NewAxis};
    use mlx_rs::transforms::eval;
    use std::collections::HashSet;

    use super::gemma2;

    let mut mdl =
        gemma2::Gemma2Model::new(&cfg).map_err(|e| format!("failed to build model: {e}"))?;
    let quantized = cfg.quantization.is_some();
    if let Some(q) = cfg.quantization.as_ref() {
        mdl = mlx_rs::nn::quantize(mdl, Some(q.group_size), Some(q.bits))
            .map_err(|e| format!("quantize wrap failed: {e}"))?;
    }
    let (group_size, bits) = cfg
        .quantization
        .as_ref()
        .map_or((64, 4), |q| (q.group_size, q.bits));
    gemma2::load_weights(&mut mdl, &dir, quantized, group_size, bits)?;

    let enc = tokenizer
        .encode(&prompt[..], true)
        .map_err(|e| format!("tokenize failed: {e}"))?;
    let prompt_ids = enc.get_ids();
    let input_tokens = prompt_ids.len() as u64;
    if prompt_ids.is_empty() {
        return Err("empty prompt after tokenization".into());
    }
    let prompt_arr = Array::from(prompt_ids).index(NewAxis);

    let initial_cache: Vec<Option<(Array, Array)>> = Vec::new();
    let out = mdl
        .forward_full(gemma2::ModelInput {
            inputs: &prompt_arr,
            cache: &initial_cache,
        })
        .map_err(|e| format!("prefill failed: {e}"))?;
    let mut cache = out.cache;
    let mut next = sample(&out.logits.index((.., -1, ..))).map_err(|e| e.to_string())?;

    let eos_set: HashSet<u32> = eos.into_iter().collect();
    let mut generated: Vec<u32> = Vec::with_capacity(4096);
    let mut emitted_prefix = String::new();

    const MAX_NEW: usize = 4096;
    for _ in 0..MAX_NEW {
        eval(std::iter::once(&next)).map_err(|e| format!("eval failed: {e}"))?;
        let id: u32 = next.item::<u32>();
        if eos_set.contains(&id) {
            break;
        }
        generated.push(id);

        if let Ok(partial) = tokenizer.decode(&generated, true) {
            if partial.contains("<end_of_turn>")
                || partial.contains("<|im_end|>")
                || partial.contains("</s>")
            {
                break;
            }
            if let Some(ref sink) = on_token
                && partial.len() > emitted_prefix.len()
                && partial.starts_with(&emitted_prefix)
            {
                let delta = &partial[emitted_prefix.len()..];
                if !delta.is_empty() {
                    sink(delta);
                    emitted_prefix = partial;
                }
            }
        }

        let tok_arr = next.index((.., NewAxis));
        let step = mdl
            .forward_full(gemma2::ModelInput {
                inputs: &tok_arr,
                cache: cache.as_slice(),
            })
            .map_err(|e| format!("decode step failed: {e}"))?;
        cache = step.cache;
        let logits = step.logits.squeeze_axes(&[1]).map_err(|e| e.to_string())?;
        next = sample(&logits).map_err(|e| e.to_string())?;
    }

    let output_tokens = generated.len() as u64;
    let mut text = tokenizer
        .decode(&generated, true)
        .map_err(|e| format!("decode failed: {e}"))?;
    for marker in ["<end_of_turn>", "<|im_end|>", "</s>"] {
        if let Some(idx) = text.find(marker) {
            text.truncate(idx);
        }
    }
    Ok((text.trim().to_string(), input_tokens, output_tokens))
}

#[cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]
#[allow(clippy::too_many_lines)]
pub async fn call_mlx(
    model: &str,
    messages: &[Message],
    on_token: Option<crate::llm::TokenSink>,
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    use crate::Role;
    use std::collections::HashSet;
    use tokenizers::Tokenizer;

    use super::{arch, gemma2, model_path, tmpl};

    let dir = model_path(model).ok_or_else(|| -> Box<dyn std::error::Error> {
        format!(
            "MLX model '{model}' not found. Pull it with `aictl --pull-mlx-model <spec>` or via `/mlx` in the REPL."
        )
        .into()
    })?;

    // --- Load config.json ---
    let cfg_path = dir.join("config.json");
    let cfg_body =
        std::fs::read_to_string(&cfg_path).map_err(|e| -> Box<dyn std::error::Error> {
            format!("failed to read {}: {e}", cfg_path.display()).into()
        })?;
    let cfg_raw: serde_json::Value = serde_json::from_str(&cfg_body)?;

    let mut is_qwen2 = false;
    let mut is_gemma2 = false;
    if let Some(arches) = cfg_raw.get("architectures").and_then(|v| v.as_array()) {
        let names: Vec<&str> = arches.iter().filter_map(|v| v.as_str()).collect();
        let supported = names.iter().any(|n| {
            matches!(
                *n,
                "LlamaForCausalLM"
                    | "Qwen2ForCausalLM"
                    | "MistralForCausalLM"
                    | "Qwen2MoeForCausalLM"
                    | "Gemma2ForCausalLM"
            )
        });
        if !supported && !names.is_empty() {
            return Err(format!(
                "unsupported model architecture: {} — only Llama-family and Gemma 2 models are supported in this build",
                names.join(", ")
            )
            .into());
        }
        is_qwen2 = names
            .iter()
            .any(|n| matches!(*n, "Qwen2ForCausalLM" | "Qwen2MoeForCausalLM"));
        is_gemma2 = names.contains(&"Gemma2ForCausalLM");
    }

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

    let prompt_text = if is_gemma2 {
        // Gemma 2's jinja template rejects system role, so we skip it
        // and build the `<start_of_turn>` envelope manually.
        tmpl::gemma_prompt(messages, reinforcement)
    } else {
        match tok_cfg.get("chat_template").and_then(|v| v.as_str()) {
            Some(template) => match tmpl::render(template, &messages_with_tail) {
                Ok(s) => s,
                Err(_) => tmpl::chatml_fallback(messages, reinforcement),
            },
            None => tmpl::chatml_fallback(messages, reinforcement),
        }
    };

    // --- Heavy work: build model, load weights, run generation ---
    let dir_for_spawn = dir.clone();
    let prompt_for_spawn = prompt_text.clone();
    let eos_for_spawn: Vec<u32> = eos_ids.into_iter().collect();

    let on_token_for_spawn = on_token.clone();
    let result = if is_gemma2 {
        // Parse the Gemma2-specific config (extra softcap / sliding-window
        // fields that the Llama config doesn't understand).
        let cfg_g: gemma2::Gemma2Config = serde_json::from_str(&cfg_body)?;
        tokio::task::spawn_blocking(move || {
            run_gemma2_inference(
                cfg_g,
                dir_for_spawn,
                prompt_for_spawn,
                eos_for_spawn,
                tokenizer,
                on_token_for_spawn,
            )
        })
        .await
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("inference task panicked: {e}").into()
        })?
    } else {
        let mut cfg: arch::LlamaConfig = serde_json::from_str(&cfg_body)?;
        // Qwen2 modeling unconditionally uses a bias on q/k/v projections.
        // Most Qwen2 config.json files omit `attention_bias`, so force it here.
        if is_qwen2 {
            cfg.attention_bias = true;
        }
        tokio::task::spawn_blocking(move || {
            run_llama_inference(
                cfg,
                dir_for_spawn,
                prompt_for_spawn,
                eos_for_spawn,
                tokenizer,
                on_token_for_spawn,
            )
        })
        .await
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("inference task panicked: {e}").into()
        })?
    };

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
    _on_token: Option<crate::llm::TokenSink>,
) -> Result<(String, TokenUsage), Box<dyn std::error::Error>> {
    if !super::host_supports_mlx() {
        return Err("MLX inference is only available on macOS + Apple Silicon (aarch64).".into());
    }
    Err("MLX inference is not compiled in. Rebuild with `cargo build --features mlx` on macOS Apple Silicon.".into())
}
