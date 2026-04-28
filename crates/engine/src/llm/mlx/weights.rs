//! Safetensors loader. Walks `model.safetensors` or `model.safetensors.index.json`,
//! rewrites `q_proj.weight` → `q_proj.inner.weight` so mlx-rs's `QuantizedLinear`
//! parameter paths match what mlx-community repos ship, and updates the model
//! in place. Shared between the Llama and Gemma2 paths.

#![cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]

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
/// `...q_proj.bias` (Qwen2-family attention bias) gets the same treatment
/// because `QuantizedLinear.inner.bias` also lives on the inner Linear.
/// `scales`/`biases` (quantization params) live directly on
/// `QuantizedLinear` and don't need rewriting.
///
/// Embeddings have the same quirk: `model.embed_tokens.weight` stays as
/// the top-level Embedding's weight in both quantized and non-quantized
/// builds, so no rewrite is needed for it. The rewrite only applies to
/// linear layers whose parent struct wraps them in `MaybeQuantized`.
pub fn translate_quantized_key(key: &str) -> String {
    // Only rewrite `<path>.weight` / `<path>.bias` when `<path>` ends in
    // a name we know corresponds to a MaybeQuantized<Linear> field.
    // Everything else passes through unchanged so the quantization
    // scales/biases and RmsNorm params land where they should.
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
    for suffix in [".weight", ".bias"] {
        if let Some(stripped) = key.strip_suffix(suffix) {
            for field in LINEAR_FIELDS {
                if stripped.ends_with(field) {
                    return format!("{stripped}.inner{suffix}");
                }
            }
        }
    }
    key.to_string()
}

/// Manually install the quantized token-embedding weights into the
/// given `MaybeQuantized<Embedding>` slot. The `ModuleParameters`
/// derive on mlx-rs's `QuantizedEmbedding` doesn't mark its inner
/// fields `#[param]` (linears with the same wrapper *do* show up —
/// apparently a quirk of the macro), so the safetensors entries
/// `model.embed_tokens.weight`, `model.embed_tokens.scales`,
/// `model.embed_tokens.biases` would otherwise be silently unused.
/// We pull them out of the merged HashMap and construct a
/// `QuantizedEmbedding` in place.
///
/// Takes the embedding field directly (not the parent model) so the
/// same helper works for both Llama-family and Gemma2 models.
pub fn install_quantized_embedding(
    embed_slot: &mut MaybeQuantized<Embedding>,
    merged: &mut HashMap<String, Array>,
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
    *embed_slot = MaybeQuantized::Quantized(qe);
    Ok(())
}

/// Walk every shard in `dir`, apply `translate_quantized_key` when the
/// model is quantized, and return a flat {translated_key → Array} map.
/// Shared between Llama and Gemma2 load paths.
pub fn build_merged_map(dir: &Path, quantized: bool) -> Result<HashMap<String, Array>, String> {
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
    Ok(merged)
}

/// Walk the model's parameter tree, consume matching entries from
/// `merged`, and report any mismatches. Shared between Llama and
/// Gemma2 load paths. Calls `eval` at the end so the lazy graph
/// materializes and the Array handles actually hold the new data.
pub fn apply_merged_to_model<M: ModuleParameters>(
    model: &mut M,
    mut merged: HashMap<String, Array>,
) -> Result<(), String> {
    let mut params = model.parameters_mut().flatten();
    let mut missing: Vec<String> = Vec::new();
    let keys: Vec<String> = params
        .keys()
        .map(std::string::ToString::to_string)
        .collect();
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

    drop(params);
    model
        .eval()
        .map_err(|e| format!("parameter eval failed: {e}"))?;
    Ok(())
}

/// Llama-family weight loader. Builds the merged shard map, installs
/// the quantized embedding, then walks the model's parameter tree.
pub fn load_model_weights(
    model: &mut LlamaModel,
    dir: &Path,
    quantized: bool,
    group_size: i32,
    bits: i32,
) -> Result<(), String> {
    let mut merged = build_merged_map(dir, quantized)?;
    // QuantizedLinear and QuantizedEmbedding call `freeze_parameters(true)`
    // on themselves during construction. Unfreeze everything before
    // updating so every param is assignable.
    model.unfreeze_parameters(true);
    if quantized {
        install_quantized_embedding(&mut model.model.embed_tokens, &mut merged, group_size, bits)?;
    }
    apply_merged_to_model(model, merged)
}
