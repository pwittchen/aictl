//! Gemma2 architecture (`Gemma2ForCausalLM`). Differs from the Llama
//! backbone in several ways that each need explicit handling:
//!
//! * **RMSNorm with `(1 + weight)`** — we wrap `fast::rms_norm` in a
//!   `GemmaRmsNorm` module that adds 1 to the stored weight at every
//!   forward pass.
//! * **Attention softcap** — the raw attention scores are run through
//!   `softcap * tanh(scores / softcap)` before the mask + softmax. That
//!   rules out mlx-rs's fused `scaled_dot_product_attention`, so
//!   attention is written out manually.
//! * **Sliding-window attention** on even-indexed layers. Odd-indexed
//!   layers use the full causal mask. Both masks are built once at the
//!   top of `forward_full` and each layer picks the one that matches
//!   its `is_sliding` flag.
//! * **Four layernorms per layer** (input / post-attn / pre-FFN /
//!   post-FFN) instead of the Llama-style two.
//! * **GeGLU MLP** with a tanh-approximate GELU (`gelu_pytorch_tanh` in
//!   HF config), implemented via `nn::gelu_approximate`.
//! * **Input embedding scaling** by `sqrt(hidden_size)` and **final
//!   logit softcap**.

#![cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]

use mlx_rs::{
    Array, array,
    builder::Builder,
    error::Exception,
    macros::{ModuleParameters, Quantizable},
    module::{Module, Param},
    nn,
    ops::{concatenate_axis, expand_dims, matmul, softmax_axis, tanh},
    quantization::MaybeQuantized,
};

use super::arch::QuantConfig;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Gemma2Config {
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
    pub quantization: Option<QuantConfig>,
    /// Applied as `softcap * tanh(scores / softcap)` to the raw
    /// attention logits. 50.0 on Gemma 2 9B/27B.
    #[serde(default)]
    pub attn_logit_softcapping: Option<f32>,
    /// Same treatment on the final logits before softmax. 30.0 on
    /// Gemma 2 9B/27B.
    #[serde(default)]
    pub final_logit_softcapping: Option<f32>,
    #[serde(default = "default_sliding_window")]
    pub sliding_window: i32,
    /// Gemma 2 decouples the attention-score scale from `head_dim`.
    /// When present it becomes `scale = 1 / sqrt(query_pre_attn_scalar)`.
    /// 224 on Gemma 2 9B, 144 on 27B, 256 on 2B.
    #[serde(default)]
    pub query_pre_attn_scalar: Option<i32>,
}

const fn default_rms_eps() -> f32 {
    1e-6
}
const fn default_rope_theta() -> f32 {
    10_000.0
}
const fn default_sliding_window() -> i32 {
    4096
}

impl Gemma2Config {
    pub fn head_dim(&self) -> i32 {
        self.head_dim
            .unwrap_or(self.hidden_size / self.num_attention_heads)
    }
    pub fn kv_heads(&self) -> i32 {
        self.num_key_value_heads.unwrap_or(self.num_attention_heads)
    }
    /// Attention scale = 1 / sqrt(query_pre_attn_scalar or head_dim).
    pub fn attn_scale(&self) -> f32 {
        let d = self
            .query_pre_attn_scalar
            .unwrap_or_else(|| self.head_dim()) as f32;
        1.0 / d.sqrt()
    }
}

/// Gemma's RMSNorm uses `(1 + weight) * rms_norm(x)` — subtly but
/// importantly different from Llama's `weight * rms_norm(x)`. We
/// keep the weight in its stored form (so safetensors keys line up
/// with the file) and add 1 at forward time. The extra add is one
/// broadcast op per norm per step, which is negligible at inference.
#[derive(Debug, Clone, ModuleParameters)]
pub struct GemmaRmsNorm {
    #[param]
    pub weight: Param<Array>,
    pub eps: f32,
}

impl GemmaRmsNorm {
    pub fn new(dim: i32, eps: f32) -> Result<Self, Exception> {
        let weight = mlx_rs::ops::zeros::<f32>(&[dim])?;
        Ok(Self {
            weight: Param::new(weight),
            eps,
        })
    }
}

impl Module<&Array> for GemmaRmsNorm {
    type Output = Array;
    type Error = Exception;

    fn forward(&mut self, x: &Array) -> Result<Array, Exception> {
        let one_plus_w = array!(1.0f32).add(self.weight.as_ref())?;
        mlx_rs::fast::rms_norm(x, &one_plus_w, self.eps)
    }

    fn training_mode(&mut self, _mode: bool) {}
}

#[derive(Debug, Clone, ModuleParameters, Quantizable)]
pub struct Attention {
    n_heads: i32,
    n_kv_heads: i32,
    head_dim: i32,
    scale: f32,
    softcap: Option<f32>,

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
    pub mask: Option<&'a Array>,
    pub cache: Option<(&'a Array, &'a Array)>,
}
pub struct AttnOut {
    pub output: Array,
    pub cache: (Array, Array),
}

impl Attention {
    pub fn new(cfg: &Gemma2Config) -> Result<Self, Exception> {
        let n_heads = cfg.num_attention_heads;
        let n_kv_heads = cfg.kv_heads();
        let head_dim = cfg.head_dim();
        let scale = cfg.attn_scale();

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
        let rope = nn::RopeBuilder::new(head_dim)
            .base(cfg.rope_theta)
            .build()?;

        Ok(Self {
            n_heads,
            n_kv_heads,
            head_dim,
            scale,
            softcap: cfg.attn_logit_softcapping,
            q_proj: MaybeQuantized::new(q_proj),
            k_proj: MaybeQuantized::new(k_proj),
            v_proj: MaybeQuantized::new(v_proj),
            o_proj: MaybeQuantized::new(o_proj),
            rope,
        })
    }

    /// Expand a grouped K/V tensor (`[b, n_kv_heads, t, head_dim]`)
    /// to `[b, n_heads, t, head_dim]` by repeating each group along a
    /// new axis. Equivalent to `repeat_interleave` on axis 1; done
    /// via `expand_dims + reshape` to avoid an explicit repeat op.
    fn expand_kv(&self, x: &Array) -> Result<Array, Exception> {
        if self.n_heads == self.n_kv_heads {
            return Ok(x.clone());
        }
        let rep = self.n_heads / self.n_kv_heads;
        let s = x.shape().to_vec();
        // [b, n_kv, t, d] -> [b, n_kv, 1, t, d]
        let e = expand_dims(x, 2)?;
        // -> [b, n_kv, rep, t, d] via broadcast
        let broadcasted = mlx_rs::ops::broadcast_to(&e, &[s[0], s[1], rep, s[2], s[3]])?;
        // -> [b, n_heads, t, d]
        broadcasted.reshape(&[s[0], self.n_heads, s[2], s[3]])
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

        // Cache the pre-expansion K/V so subsequent steps reuse the
        // compact GQA-sized tensor. Expand only for the attention math.
        let k_full = self.expand_kv(&k)?;
        let v_full = self.expand_kv(&v)?;

        let k_t = k_full.transpose_axes(&[0, 1, 3, 2])?;
        let mut scores = matmul(&q, &k_t)?.multiply(array!(self.scale))?;

        if let Some(cap) = self.softcap {
            let c = array!(cap);
            let ratio = scores.divide(&c)?;
            scores = tanh(&ratio)?.multiply(&c)?;
        }

        if let Some(m) = mask {
            scores = scores.add(m)?;
        }

        let weights = softmax_axis(&scores, -1, None)?;
        let out = matmul(&weights, &v_full)?;
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
    pub fn new(cfg: &Gemma2Config) -> Result<Self, Exception> {
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
        // Gemma2 uses gelu_pytorch_tanh on the gate branch.
        let gate = nn::gelu_approximate(self.gate_proj.forward(x)?)?;
        let up = self.up_proj.forward(x)?;
        self.down_proj.forward(&gate.multiply(&up)?)
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
    pub input_layernorm: GemmaRmsNorm,
    #[param]
    pub post_attention_layernorm: GemmaRmsNorm,
    #[param]
    pub pre_feedforward_layernorm: GemmaRmsNorm,
    #[param]
    pub post_feedforward_layernorm: GemmaRmsNorm,
    /// HF's Gemma 2 modeling uses `is_sliding = !(layer_idx % 2)`,
    /// so even-indexed layers get the sliding-window mask and odd
    /// layers get the full causal mask.
    pub is_sliding: bool,
}

impl Layer {
    pub fn new(cfg: &Gemma2Config, layer_idx: i32) -> Result<Self, Exception> {
        Ok(Self {
            self_attn: Attention::new(cfg)?,
            mlp: Mlp::new(cfg)?,
            input_layernorm: GemmaRmsNorm::new(cfg.hidden_size, cfg.rms_norm_eps)?,
            post_attention_layernorm: GemmaRmsNorm::new(cfg.hidden_size, cfg.rms_norm_eps)?,
            pre_feedforward_layernorm: GemmaRmsNorm::new(cfg.hidden_size, cfg.rms_norm_eps)?,
            post_feedforward_layernorm: GemmaRmsNorm::new(cfg.hidden_size, cfg.rms_norm_eps)?,
            is_sliding: layer_idx % 2 == 0,
        })
    }
}

pub struct LayerIn<'a> {
    pub x: &'a Array,
    pub mask_global: Option<&'a Array>,
    pub mask_sliding: Option<&'a Array>,
    pub cache: Option<(&'a Array, &'a Array)>,
}

impl Module<LayerIn<'_>> for Layer {
    type Output = AttnOut;
    type Error = Exception;

    fn forward(&mut self, input: LayerIn<'_>) -> Result<Self::Output, Self::Error> {
        let LayerIn {
            x,
            mask_global,
            mask_sliding,
            cache,
        } = input;
        let mask = if self.is_sliding {
            mask_sliding
        } else {
            mask_global
        };

        // Attention: x -> input_norm -> attn -> post_attn_norm -> residual.
        let h_norm = self.input_layernorm.forward(x)?;
        let attn = self.self_attn.forward(AttnIn {
            x: &h_norm,
            mask,
            cache,
        })?;
        let attn_out = self.post_attention_layernorm.forward(&attn.output)?;
        let h = x.add(&attn_out)?;

        // MLP: h -> pre_ffn_norm -> mlp -> post_ffn_norm -> residual.
        let r = self.pre_feedforward_layernorm.forward(&h)?;
        let r = self.mlp.forward(&r)?;
        let r = self.post_feedforward_layernorm.forward(&r)?;
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
        self.pre_feedforward_layernorm.training_mode(mode);
        self.post_feedforward_layernorm.training_mode(mode);
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
    pub norm: GemmaRmsNorm,
}

#[derive(Debug, Clone, ModuleParameters, Quantizable)]
pub struct Gemma2Model {
    #[quantizable]
    #[param]
    pub model: Backbone,
    pub embed_scale: f32,
    pub final_softcap: Option<f32>,
    pub sliding_window: i32,
}

pub struct ModelInput<'a> {
    pub inputs: &'a Array,
    pub cache: &'a [Option<(Array, Array)>],
}
pub struct ModelOutput {
    pub logits: Array,
    pub cache: Vec<Option<(Array, Array)>>,
}

impl Gemma2Model {
    pub fn new(cfg: &Gemma2Config) -> Result<Self, Exception> {
        let embed_tokens = nn::Embedding::new(cfg.vocab_size, cfg.hidden_size)?;
        let layers = (0..cfg.num_hidden_layers)
            .map(|i| Layer::new(cfg, i))
            .collect::<Result<Vec<_>, _>>()?;
        let norm = GemmaRmsNorm::new(cfg.hidden_size, cfg.rms_norm_eps)?;
        Ok(Self {
            model: Backbone {
                embed_tokens: MaybeQuantized::new(embed_tokens),
                layers,
                norm,
            },
            embed_scale: (cfg.hidden_size as f32).sqrt(),
            final_softcap: cfg.final_logit_softcapping,
            sliding_window: cfg.sliding_window,
        })
    }

    pub fn forward_full(&mut self, input: ModelInput<'_>) -> Result<ModelOutput, Exception> {
        let ModelInput { inputs, cache } = input;
        let mut h = self.model.embed_tokens.forward(inputs)?;
        // Gemma2 scales input embeddings by sqrt(hidden_size).
        h = h.multiply(array!(self.embed_scale))?;

        let seq_len = h.shape()[1];
        let past_len = cache
            .first()
            .and_then(Option::as_ref)
            .map_or(0, |(k, _)| k.shape()[2]);

        // Build the two masks once. For single-token decode when
        // past_len stays below sliding_window, both masks collapse to
        // "allow all past keys" and we can skip building them.
        let need_mask = seq_len > 1 || past_len + seq_len > self.sliding_window;
        let mask_global = if need_mask {
            Some(build_causal_mask(seq_len, past_len, None, h.dtype())?)
        } else {
            None
        };
        let mask_sliding = if need_mask {
            Some(build_causal_mask(
                seq_len,
                past_len,
                Some(self.sliding_window),
                h.dtype(),
            )?)
        } else {
            None
        };

        let mut out_cache = Vec::with_capacity(self.model.layers.len());
        for (i, layer) in self.model.layers.iter_mut().enumerate() {
            let entry = cache.get(i).and_then(Option::as_ref).map(|(k, v)| (k, v));
            let out = layer.forward(LayerIn {
                x: &h,
                mask_global: mask_global.as_ref(),
                mask_sliding: mask_sliding.as_ref(),
                cache: entry,
            })?;
            h = out.output;
            out_cache.push(Some(out.cache));
        }

        let h = self.model.norm.forward(&h)?;
        // Gemma 2 ties the output projection to embed_tokens.
        let mut logits = match &self.model.embed_tokens {
            MaybeQuantized::Original(e) => e.as_linear(&h)?,
            MaybeQuantized::Quantized(q) => q.as_linear(&h)?,
        };
        if let Some(cap) = self.final_softcap {
            let c = array!(cap);
            let ratio = logits.divide(&c)?;
            logits = tanh(&ratio)?.multiply(&c)?;
        }

        Ok(ModelOutput {
            logits,
            cache: out_cache,
        })
    }
}

/// Build an additive causal mask with optional sliding-window limit.
/// Shape is `[1, 1, q_len, past_len + q_len]` so it broadcasts over
/// batch and heads. Masked entries are `-inf`; allowed entries are
/// `0`. When `window` is `Some(w)`, keys older than `abs_pos - w + 1`
/// are also masked, i.e. each query only attends to the last `w`
/// keys.
fn build_causal_mask(
    q_len: i32,
    past_len: i32,
    window: Option<i32>,
    dtype: mlx_rs::Dtype,
) -> Result<Array, Exception> {
    let k_len = past_len + q_len;
    let mut data = vec![0f32; (q_len * k_len) as usize];
    for i in 0..q_len {
        let abs = past_len + i;
        let lower = window.map_or(0, |w| (abs - w + 1).max(0));
        for j in 0..k_len {
            if j > abs || j < lower {
                data[(i * k_len + j) as usize] = f32::NEG_INFINITY;
            }
        }
    }
    let arr = Array::from_slice(&data, &[1, 1, q_len, k_len]);
    arr.as_dtype(dtype)
}

/// Gemma2 weight loader. Mirrors `weights::load_model_weights` but
/// operates on `Gemma2Model` and installs its embedding slot.
pub fn load_weights(
    model: &mut Gemma2Model,
    dir: &std::path::Path,
    quantized: bool,
    group_size: i32,
    bits: i32,
) -> Result<(), String> {
    use super::weights::{apply_merged_to_model, build_merged_map, install_quantized_embedding};
    use mlx_rs::module::ModuleParameters;
    let mut merged = build_merged_map(dir, quantized)?;
    model.unfreeze_parameters(true);
    if quantized {
        install_quantized_embedding(&mut model.model.embed_tokens, &mut merged, group_size, bits)?;
    }
    apply_merged_to_model(model, merged)
}
