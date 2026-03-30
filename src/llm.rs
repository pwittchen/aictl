/// Available models: (provider_str, model_name, api_key_config_key)
pub const MODELS: &[(&str, &str, &str)] = &[
    ("anthropic", "claude-haiku-4-20250414", "ANTHROPIC_API_KEY"),
    ("anthropic", "claude-sonnet-4-20250514", "ANTHROPIC_API_KEY"),
    ("anthropic", "claude-opus-4-20250514", "ANTHROPIC_API_KEY"),
    ("openai", "gpt-4.1-nano", "OPENAI_API_KEY"),
    ("openai", "gpt-4.1-mini", "OPENAI_API_KEY"),
    ("openai", "gpt-4.1", "OPENAI_API_KEY"),
    ("openai", "gpt-4o-mini", "OPENAI_API_KEY"),
    ("openai", "gpt-4o", "OPENAI_API_KEY"),
    ("openai", "o4-mini", "OPENAI_API_KEY"),
];

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Returns (input, output) price per million tokens for known models.
fn price_per_million(model: &str) -> Option<(f64, f64)> {
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

    None
}

/// Returns the context window size (max input tokens) for known models.
pub fn context_limit(model: &str) -> u64 {
    if model.starts_with("gpt-4.1") {
        return 200_000;
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
    128_000
}

impl TokenUsage {
    /// Estimate cost in USD. Returns None if the model is unknown.
    pub fn estimate_cost(&self, model: &str) -> Option<f64> {
        let (input_ppm, output_ppm) = price_per_million(model)?;
        let cost = (self.input_tokens as f64 * input_ppm + self.output_tokens as f64 * output_ppm)
            / 1_000_000.0;
        Some(cost)
    }
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
    fn price_claude_haiku_4() {
        let (i, o) = price_per_million("claude-haiku-4-20250414").unwrap();
        assert_eq!(i, 1.00);
        assert_eq!(o, 5.00);
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
        };
        let cost = usage.estimate_cost("gpt-4.1").unwrap();
        assert!((cost - 0.0).abs() < 1e-9);
    }

    #[test]
    fn estimate_cost_unknown_model() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 100,
        };
        assert!(usage.estimate_cost("unknown-model").is_none());
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
    fn context_limit_unknown_defaults() {
        assert_eq!(context_limit("unknown-model"), 128_000);
    }
}
