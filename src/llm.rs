/// Available models: (`provider_str`, `model_name`, `api_key_config_key`)
pub const MODELS: &[(&str, &str, &str)] = &[
    (
        "anthropic",
        "claude-haiku-4-5-20251001",
        "LLM_ANTHROPIC_API_KEY",
    ),
    (
        "anthropic",
        "claude-sonnet-4-20250514",
        "LLM_ANTHROPIC_API_KEY",
    ),
    ("anthropic", "claude-sonnet-4-6", "LLM_ANTHROPIC_API_KEY"),
    (
        "anthropic",
        "claude-opus-4-20250514",
        "LLM_ANTHROPIC_API_KEY",
    ),
    ("anthropic", "claude-opus-4-6", "LLM_ANTHROPIC_API_KEY"),
    ("openai", "gpt-4.1-nano", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-4.1-mini", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-4.1", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-4o-mini", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-4o", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5-mini", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5.2", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5.2-pro", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5.4-nano", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5.4-mini", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5.4", "LLM_OPENAI_API_KEY"),
    ("openai", "gpt-5.4-pro", "LLM_OPENAI_API_KEY"),
    ("openai", "o4-mini", "LLM_OPENAI_API_KEY"),
    ("openai", "o3", "LLM_OPENAI_API_KEY"),
    ("openai", "o1", "LLM_OPENAI_API_KEY"),
    ("gemini", "gemini-2.5-pro", "LLM_GEMINI_API_KEY"),
    ("gemini", "gemini-2.5-flash", "LLM_GEMINI_API_KEY"),
    ("gemini", "gemini-3.1-pro-preview", "LLM_GEMINI_API_KEY"),
    (
        "gemini",
        "gemini-3.1-flash-lite-preview",
        "LLM_GEMINI_API_KEY",
    ),
    ("grok", "grok-3", "LLM_GROK_API_KEY"),
    ("grok", "grok-3-mini", "LLM_GROK_API_KEY"),
    ("grok", "grok-4", "LLM_GROK_API_KEY"),
    ("grok", "grok-4-fast-reasoning", "LLM_GROK_API_KEY"),
    ("grok", "grok-4-fast-non-reasoning", "LLM_GROK_API_KEY"),
    ("grok", "grok-4-1-fast-reasoning", "LLM_GROK_API_KEY"),
    ("grok", "grok-4-1-fast-non-reasoning", "LLM_GROK_API_KEY"),
    ("mistral", "mistral-large-latest", "LLM_MISTRAL_API_KEY"),
    ("mistral", "mistral-medium-latest", "LLM_MISTRAL_API_KEY"),
    ("mistral", "mistral-small-latest", "LLM_MISTRAL_API_KEY"),
    ("mistral", "codestral-latest", "LLM_MISTRAL_API_KEY"),
    ("deepseek", "deepseek-chat", "LLM_DEEPSEEK_API_KEY"),
    ("deepseek", "deepseek-reasoner", "LLM_DEEPSEEK_API_KEY"),
    ("kimi", "kimi-k2.5", "LLM_KIMI_API_KEY"),
    ("kimi", "kimi-k2-0905-preview", "LLM_KIMI_API_KEY"),
    ("kimi", "kimi-k2-0711-preview", "LLM_KIMI_API_KEY"),
    ("kimi", "kimi-k2-turbo-preview", "LLM_KIMI_API_KEY"),
    ("kimi", "kimi-k2-thinking", "LLM_KIMI_API_KEY"),
    ("kimi", "kimi-k2-thinking-turbo", "LLM_KIMI_API_KEY"),
    ("kimi", "moonshot-v1-128k", "LLM_KIMI_API_KEY"),
    ("kimi", "moonshot-v1-32k", "LLM_KIMI_API_KEY"),
    ("kimi", "moonshot-v1-8k", "LLM_KIMI_API_KEY"),
    ("zai", "glm-5.1", "LLM_ZAI_API_KEY"),
    ("zai", "glm-5-turbo", "LLM_ZAI_API_KEY"),
    ("zai", "glm-5", "LLM_ZAI_API_KEY"),
    ("zai", "glm-4.7", "LLM_ZAI_API_KEY"),
    ("zai", "glm-4.7-flash", "LLM_ZAI_API_KEY"),
];

#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_field_names)]
pub struct TokenUsage {
    /// Fresh (non-cached) input tokens billed at the full input price.
    /// For providers where the raw usage field (e.g. `OpenAI`'s `prompt_tokens`)
    /// *includes* cached tokens, callers must subtract the cached count before
    /// populating this field so that `cache_read` tokens are not double-billed.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Tokens written to the provider's prompt cache (Anthropic: 1.25× input price).
    /// Only populated by Anthropic; other providers lack an explicit
    /// cache-write accounting.
    pub cache_creation_input_tokens: u64,
    /// Tokens read from the provider's prompt cache. Billed at
    /// `cache_read_multiplier(model) × input price`.
    pub cache_read_input_tokens: u64,
}

/// Returns the cache-read price multiplier for a model, applied on top of
/// the base input price. Covers provider-specific discounts so the cost meter
/// reflects what the wallet actually pays when cached tokens are reported.
///
/// Defaults to 1.0 (no discount) for unknown models — conservative, but keeps
/// the meter from silently under-billing. Rates are approximate public list
/// prices as of early 2026 and may drift; adjust as providers change pricing.
fn cache_read_multiplier(model: &str) -> f64 {
    // Anthropic — 10% of input (ephemeral cache hit)
    if model.contains("claude")
        || model.contains("opus")
        || model.contains("sonnet")
        || model.contains("haiku")
    {
        return 0.1;
    }

    // OpenAI GPT-5: 10% of input
    if model.starts_with("gpt-5") {
        return 0.1;
    }
    // OpenAI GPT-4.1 and o-series reasoning: 25% of input
    if model.starts_with("gpt-4.1")
        || model.starts_with("o4")
        || model.starts_with("o3")
        || model.starts_with("o1")
    {
        return 0.25;
    }
    // OpenAI GPT-4o: 50% of input
    if model.starts_with("gpt-4o") {
        return 0.5;
    }

    // Google Gemini implicit caching: 25% of input
    if model.starts_with("gemini-") {
        return 0.25;
    }

    // xAI Grok: 25% of input
    if model.starts_with("grok-") {
        return 0.25;
    }

    // DeepSeek cache hit: ~26% of input (treat as 25%)
    if model.starts_with("deepseek-") {
        return 0.25;
    }

    // Moonshot Kimi: approximate at 25%
    if model.starts_with("kimi-") || model.starts_with("moonshot-") {
        return 0.25;
    }

    // Unknown — charge cache reads at full input price
    1.0
}

/// Returns (input, output) price per million tokens for known models.
#[allow(clippy::too_many_lines)]
fn price_per_million(model: &str) -> Option<(f64, f64)> {
    // OpenAI — GPT-5.4 (current flagship; dual-tier pricing above 272K
    // context — these are the short-context rates)
    if model.starts_with("gpt-5.4-nano") {
        return Some((0.20, 1.25));
    }
    if model.starts_with("gpt-5.4-mini") {
        return Some((0.75, 4.50));
    }
    if model.starts_with("gpt-5.4-pro") {
        return Some((60.00, 270.00));
    }
    if model.starts_with("gpt-5.4") {
        return Some((2.50, 15.00));
    }

    // OpenAI — GPT-5.2
    if model.starts_with("gpt-5.2-pro") {
        return Some((30.00, 180.00));
    }
    if model.starts_with("gpt-5.2") {
        return Some((1.75, 14.00));
    }

    // OpenAI — GPT-5
    if model.starts_with("gpt-5-mini") {
        return Some((0.25, 2.00));
    }
    if model.starts_with("gpt-5") {
        return Some((1.25, 10.00));
    }

    // OpenAI — GPT-4.1
    if model.starts_with("gpt-4.1-nano") {
        return Some((0.10, 0.40));
    }
    if model.starts_with("gpt-4.1-mini") {
        return Some((0.40, 1.60));
    }
    if model.starts_with("gpt-4.1") {
        return Some((2.00, 8.00));
    }

    // OpenAI — GPT-4o
    if model.starts_with("gpt-4o-mini") {
        return Some((0.15, 0.60));
    }
    if model.starts_with("gpt-4o") {
        return Some((2.50, 10.00));
    }

    // OpenAI — o-series reasoning models
    if model.starts_with("o4-mini") {
        return Some((1.10, 4.40));
    }
    if model.starts_with("o3") {
        return Some((2.00, 8.00));
    }
    if model.starts_with("o1") {
        return Some((15.00, 60.00));
    }

    // Anthropic — opus 4.5+ ($5/$25), older opus 4/4.1 ($15/$75)
    if model.contains("opus-4-5") || model.contains("opus-4-6") {
        return Some((5.00, 25.00));
    }
    if model.contains("opus-4") {
        return Some((15.00, 75.00));
    }

    // Anthropic — all sonnet versions
    if model.contains("sonnet") {
        return Some((3.00, 15.00));
    }

    // Anthropic — haiku 4.5+
    if model.contains("haiku-4") {
        return Some((1.00, 5.00));
    }
    // Anthropic — haiku 3 (legacy)
    if model.contains("haiku") {
        return Some((0.25, 1.25));
    }

    // Google Gemini — 3.1 (dual-tier pricing above 200K context;
    // these are the short-context rates)
    if model.starts_with("gemini-3.1-flash-lite") {
        return Some((0.25, 1.50));
    }
    if model.starts_with("gemini-3.1-pro") {
        return Some((2.00, 12.00));
    }
    // Google Gemini — 2.5
    if model.starts_with("gemini-2.5-pro") {
        return Some((1.25, 10.00));
    }
    if model.starts_with("gemini-2.5-flash") {
        return Some((0.15, 0.60));
    }

    // xAI Grok — 4 family (4.x Fast variants share pricing)
    if model.starts_with("grok-4-fast") || model.starts_with("grok-4-1-fast") {
        return Some((0.20, 0.50));
    }
    if model.starts_with("grok-4") {
        return Some((3.00, 15.00));
    }
    // xAI Grok — 3
    if model.starts_with("grok-3-mini") {
        return Some((0.30, 0.50));
    }
    if model.starts_with("grok-3") {
        return Some((3.00, 15.00));
    }

    // Mistral
    if model.starts_with("mistral-large") {
        return Some((2.00, 6.00));
    }
    if model.starts_with("mistral-medium") {
        return Some((0.40, 2.00));
    }
    if model.starts_with("mistral-small") {
        return Some((0.10, 0.30));
    }
    if model.starts_with("codestral") {
        return Some((0.30, 0.90));
    }

    // DeepSeek
    if model.starts_with("deepseek-reasoner") {
        return Some((0.55, 2.19));
    }
    if model.starts_with("deepseek-chat") {
        return Some((0.27, 1.10));
    }

    // Kimi
    if model.starts_with("kimi-k2") || model.starts_with("kimi-k2.5") {
        return Some((0.60, 2.00));
    }
    if model.starts_with("moonshot-v1") {
        return Some((0.60, 2.00));
    }

    // Z.ai — order matters: more specific prefixes first
    if model.starts_with("glm-5.1") {
        return Some((1.40, 4.40));
    }
    if model.starts_with("glm-5-turbo") {
        return Some((1.20, 4.00));
    }
    if model.starts_with("glm-5") {
        return Some((0.72, 2.30));
    }
    if model.starts_with("glm-4.7-flash") {
        return Some((0.06, 0.40));
    }
    if model.starts_with("glm-4.7") {
        return Some((0.39, 1.75));
    }

    None
}

/// Returns the context window size (max input tokens) for known models.
pub fn context_limit(model: &str) -> u64 {
    if model.starts_with("gpt-4.1") {
        return 200_000;
    }
    if model.starts_with("gpt-5.4") {
        return 1_000_000;
    }
    if model.starts_with("gpt-4o") || model.starts_with("gpt-5") {
        return 128_000;
    }
    if model.starts_with("o4-mini") || model.starts_with("o3") || model.starts_with("o1") {
        return 200_000;
    }
    if model.contains("claude-") || model.contains("claude") {
        return 200_000;
    }
    if model.starts_with("gemini-3.1-pro") || model.starts_with("gemini-3.1-flash-lite") {
        return 1_000_000;
    }
    if model.starts_with("gemini-") {
        return 200_000;
    }
    if model.starts_with("grok-4-fast") || model.starts_with("grok-4-1-fast") {
        return 2_000_000;
    }
    if model.starts_with("grok-4") {
        return 256_000;
    }
    if model.starts_with("grok-") {
        return 131_072;
    }
    if model.starts_with("mistral-") || model.starts_with("codestral") {
        return 128_000;
    }
    if model.starts_with("deepseek-") {
        return 128_000;
    }
    if model == "kimi-k2-0711-preview" {
        return 128_000;
    }
    if model.starts_with("kimi-") {
        return 256_000;
    }
    if model == "moonshot-v1-8k" || model == "moonshot-v1-8k-vision-preview" {
        return 8_000;
    }
    if model == "moonshot-v1-32k" || model == "moonshot-v1-32k-vision-preview" {
        return 32_000;
    }
    if model.starts_with("moonshot-v1-128k") {
        return 128_000;
    }
    if model.starts_with("glm-") {
        return 203_000;
    }
    128_000
}

impl TokenUsage {
    /// Estimate cost in USD. Returns None if the model is unknown.
    #[allow(clippy::cast_precision_loss)]
    pub fn estimate_cost(&self, model: &str) -> Option<f64> {
        let (input_ppm, output_ppm) = price_per_million(model)?;
        let read_mult = cache_read_multiplier(model);
        let base_input = self.input_tokens as f64 * input_ppm;
        let cache_write = self.cache_creation_input_tokens as f64 * input_ppm * 1.25;
        let cache_read = self.cache_read_input_tokens as f64 * input_ppm * read_mult;
        let output = self.output_tokens as f64 * output_ppm;
        let cost = (base_input + cache_write + cache_read + output) / 1_000_000.0;
        Some(cost)
    }
}

/// Compute a percentage (0–100) from a part/total pair.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn pct(part: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    (part as f64 / total as f64 * 100.0).min(100.0) as u8
}

/// Compute a percentage (0–100) from usize values.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn pct_usize(part: usize, total: usize) -> u8 {
    if total == 0 {
        return 0;
    }
    (part as f64 / total as f64 * 100.0).min(100.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- price_per_million ---

    #[test]
    fn price_gpt4_1() {
        let (i, o) = price_per_million("gpt-4.1").unwrap();
        assert_eq!(i, 2.00);
        assert_eq!(o, 8.00);
    }

    #[test]
    fn price_gpt4_1_mini() {
        let (i, o) = price_per_million("gpt-4.1-mini").unwrap();
        assert_eq!(i, 0.40);
        assert_eq!(o, 1.60);
    }

    #[test]
    fn price_gpt4_1_nano() {
        let (i, o) = price_per_million("gpt-4.1-nano").unwrap();
        assert_eq!(i, 0.10);
        assert_eq!(o, 0.40);
    }

    #[test]
    fn price_gpt4o() {
        let (i, o) = price_per_million("gpt-4o").unwrap();
        assert_eq!(i, 2.50);
        assert_eq!(o, 10.00);
    }

    #[test]
    fn price_gpt4o_mini() {
        let (i, o) = price_per_million("gpt-4o-mini").unwrap();
        assert_eq!(i, 0.15);
        assert_eq!(o, 0.60);
    }

    #[test]
    fn price_o4_mini() {
        let (i, o) = price_per_million("o4-mini").unwrap();
        assert_eq!(i, 1.10);
        assert_eq!(o, 4.40);
    }

    #[test]
    fn price_claude_sonnet() {
        let (i, o) = price_per_million("claude-sonnet-4-20250514").unwrap();
        assert_eq!(i, 3.00);
        assert_eq!(o, 15.00);
    }

    #[test]
    fn price_claude_opus_4() {
        let (i, o) = price_per_million("claude-opus-4-20250514").unwrap();
        assert_eq!(i, 15.00);
        assert_eq!(o, 75.00);
    }

    #[test]
    fn price_claude_opus_4_6() {
        let (i, o) = price_per_million("claude-opus-4-6").unwrap();
        assert_eq!(i, 5.00);
        assert_eq!(o, 25.00);
    }

    #[test]
    fn price_claude_haiku_4() {
        let (i, o) = price_per_million("claude-haiku-4-5-20251001").unwrap();
        assert_eq!(i, 1.00);
        assert_eq!(o, 5.00);
    }

    #[test]
    fn price_gpt5_2() {
        let (i, o) = price_per_million("gpt-5.2").unwrap();
        assert_eq!(i, 1.75);
        assert_eq!(o, 14.00);
    }

    #[test]
    fn price_gpt5_4_family() {
        let (i, o) = price_per_million("gpt-5.4").unwrap();
        assert_eq!(i, 2.50);
        assert_eq!(o, 15.00);
        let (i, o) = price_per_million("gpt-5.4-mini").unwrap();
        assert_eq!(i, 0.75);
        assert_eq!(o, 4.50);
        let (i, o) = price_per_million("gpt-5.4-nano").unwrap();
        assert_eq!(i, 0.20);
        assert_eq!(o, 1.25);
    }

    #[test]
    fn price_gemini_3_1_pro() {
        let (i, o) = price_per_million("gemini-3.1-pro-preview").unwrap();
        assert_eq!(i, 2.00);
        assert_eq!(o, 12.00);
    }

    #[test]
    fn price_grok_4() {
        let (i, o) = price_per_million("grok-4").unwrap();
        assert_eq!(i, 3.00);
        assert_eq!(o, 15.00);
        let (i, o) = price_per_million("grok-4-fast-reasoning").unwrap();
        assert_eq!(i, 0.20);
        assert_eq!(o, 0.50);
    }

    #[test]
    fn price_glm_5_1_and_turbo() {
        let (i, o) = price_per_million("glm-5.1").unwrap();
        assert_eq!(i, 1.40);
        assert_eq!(o, 4.40);
        let (i, o) = price_per_million("glm-5-turbo").unwrap();
        assert_eq!(i, 1.20);
        assert_eq!(o, 4.00);
        // existing glm-5 bucket must still match plain "glm-5"
        let (i, o) = price_per_million("glm-5").unwrap();
        assert_eq!(i, 0.72);
        assert_eq!(o, 2.30);
    }

    #[test]
    fn price_unknown_model() {
        assert!(price_per_million("unknown-model-xyz").is_none());
    }

    // --- estimate_cost ---

    #[test]
    fn estimate_cost_known_model() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        let cost = usage.estimate_cost("gpt-4.1").unwrap();
        // 1M * 2.00 / 1M + 1M * 8.00 / 1M = 10.00
        assert!((cost - 10.00).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_zero_tokens() {
        let usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            ..TokenUsage::default()
        };
        let cost = usage.estimate_cost("gpt-4.1").unwrap();
        assert!((cost - 0.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_unknown_model() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 100,
            ..TokenUsage::default()
        };
        assert!(usage.estimate_cost("unknown-model").is_none());
    }

    #[test]
    fn estimate_cost_with_cache() {
        // Sonnet: $3/M input, $15/M output
        // Cache write: 1.25× input = $3.75/M
        // Cache read: 0.1× input = $0.30/M
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 1_000_000,
            cache_read_input_tokens: 1_000_000,
        };
        let cost = usage.estimate_cost("claude-sonnet-4-20250514").unwrap();
        // 1M * 3.00 + 1M * 3.00 * 1.25 + 1M * 3.00 * 0.1 + 1M * 15.00 = 3 + 3.75 + 0.3 + 15 = 22.05
        assert!((cost - 22.05).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_openai_cache_read() {
        // gpt-4.1-mini: $0.40/M input, $1.60/M output, cache read = 25%
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_input_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        let cost = usage.estimate_cost("gpt-4.1-mini").unwrap();
        // 1M * 0.40 + 1M * 0.40 * 0.25 + 1M * 1.60 = 0.40 + 0.10 + 1.60 = 2.10
        assert!((cost - 2.10).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_deepseek_cache_read() {
        // deepseek-chat: $0.27/M input, $1.10/M output, cache read = 25%
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_input_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        let cost = usage.estimate_cost("deepseek-chat").unwrap();
        // 1M * 0.27 + 1M * 0.27 * 0.25 + 1M * 1.10 = 0.27 + 0.0675 + 1.10 = 1.4375
        assert!((cost - 1.4375).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_gemini_cache_read() {
        // gemini-2.5-flash: $0.15/M input, $0.60/M output, cache read = 25%
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_input_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        let cost = usage.estimate_cost("gemini-2.5-flash").unwrap();
        // 1M * 0.15 + 1M * 0.15 * 0.25 + 1M * 0.60 = 0.15 + 0.0375 + 0.60 = 0.7875
        assert!((cost - 0.7875).abs() < 1e-9);
    }

    #[test]
    fn cache_read_multiplier_anthropic() {
        assert!((cache_read_multiplier("claude-sonnet-4-20250514") - 0.1).abs() < 1e-9);
        assert!((cache_read_multiplier("claude-opus-4-20250514") - 0.1).abs() < 1e-9);
        assert!((cache_read_multiplier("claude-haiku-4-5-20251001") - 0.1).abs() < 1e-9);
    }

    #[test]
    fn cache_read_multiplier_openai_family() {
        assert!((cache_read_multiplier("gpt-5") - 0.1).abs() < 1e-9);
        assert!((cache_read_multiplier("gpt-4.1") - 0.25).abs() < 1e-9);
        assert!((cache_read_multiplier("gpt-4.1-mini") - 0.25).abs() < 1e-9);
        assert!((cache_read_multiplier("o4-mini") - 0.25).abs() < 1e-9);
        assert!((cache_read_multiplier("gpt-4o") - 0.5).abs() < 1e-9);
        assert!((cache_read_multiplier("gpt-4o-mini") - 0.5).abs() < 1e-9);
    }

    #[test]
    fn cache_read_multiplier_other_providers() {
        assert!((cache_read_multiplier("gemini-2.5-flash") - 0.25).abs() < 1e-9);
        assert!((cache_read_multiplier("grok-3") - 0.25).abs() < 1e-9);
        assert!((cache_read_multiplier("deepseek-chat") - 0.25).abs() < 1e-9);
        assert!((cache_read_multiplier("kimi-k2-0905-preview") - 0.25).abs() < 1e-9);
    }

    #[test]
    fn cache_read_multiplier_unknown_is_full_price() {
        assert!((cache_read_multiplier("unknown-model") - 1.0).abs() < 1e-9);
    }

    // --- context_limit ---

    #[test]
    fn context_limit_gpt4_1() {
        assert_eq!(context_limit("gpt-4.1"), 200_000);
        assert_eq!(context_limit("gpt-4.1-mini"), 200_000);
    }

    #[test]
    fn context_limit_gpt4o() {
        assert_eq!(context_limit("gpt-4o"), 128_000);
        assert_eq!(context_limit("gpt-4o-mini"), 128_000);
    }

    #[test]
    fn context_limit_o4_mini() {
        assert_eq!(context_limit("o4-mini"), 200_000);
    }

    #[test]
    fn context_limit_claude() {
        assert_eq!(context_limit("claude-sonnet-4-20250514"), 200_000);
        assert_eq!(context_limit("claude-opus-4-20250514"), 200_000);
    }

    #[test]
    fn context_limit_gpt5_4_is_1m() {
        assert_eq!(context_limit("gpt-5.4"), 1_000_000);
        assert_eq!(context_limit("gpt-5.4-mini"), 1_000_000);
        // gpt-5 stays at 128K
        assert_eq!(context_limit("gpt-5"), 128_000);
    }

    #[test]
    fn context_limit_gemini_3_1_pro_is_1m() {
        assert_eq!(context_limit("gemini-3.1-pro-preview"), 1_000_000);
        assert_eq!(context_limit("gemini-3.1-flash-lite-preview"), 1_000_000);
        // older gemini stays at 200K
        assert_eq!(context_limit("gemini-2.5-pro"), 200_000);
    }

    #[test]
    fn context_limit_grok_4() {
        assert_eq!(context_limit("grok-4"), 256_000);
        assert_eq!(context_limit("grok-4-fast-reasoning"), 2_000_000);
        assert_eq!(context_limit("grok-4-1-fast-reasoning"), 2_000_000);
        assert_eq!(context_limit("grok-3"), 131_072);
    }

    #[test]
    fn context_limit_unknown_defaults() {
        assert_eq!(context_limit("unknown-model"), 128_000);
    }
}
