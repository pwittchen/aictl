//! HF-naming Llama-family transformer built on `mlx-rs` primitives. Works for
//! Llama 3.x, Qwen 2.5, Mistral 7B v0.3, and DeepSeek-R1 Distill Qwen. Does NOT
//! handle Phi (different MLP) or MoE models.

#![cfg(all(feature = "mlx", target_os = "macos", target_arch = "aarch64"))]

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
    /// Whether q/k/v projections carry a bias term. Qwen2-family configs
    /// don't always set this field (the Python side forces it to true in
    /// modeling code), so we override it from the architecture name in
    /// `call_mlx` before constructing the model.
    #[serde(default)]
    pub attention_bias: bool,
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
            .bias(cfg.attention_bias)
            .build()?;
        let k_proj = nn::LinearBuilder::new(cfg.hidden_size, n_kv_heads * head_dim)
            .bias(cfg.attention_bias)
            .build()?;
        let v_proj = nn::LinearBuilder::new(cfg.hidden_size, n_kv_heads * head_dim)
            .bias(cfg.attention_bias)
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
    /// `model.embed_tokens` for the output projection. Marked
    /// `#[quantizable]` so it rides the top-level `mlx_rs::nn::quantize`
    /// call along with the rest of the model — without this, an otherwise
    /// quantized model would keep `lm_head` as a plain `Linear`, and the
    /// file's quantized `lm_head.weight / scales / biases` would all end
    /// up unused while the model's `lm_head.weight` slot stayed unfilled.
    #[quantizable]
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
            let m = nn::MultiHeadAttention::create_additive_causal_mask::<f32>(h.shape()[1])?;
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
