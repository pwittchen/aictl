pub mod anthropic;
pub mod openai;

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

impl TokenUsage {
    /// Estimate cost in USD. Returns None if the model is unknown.
    pub fn estimate_cost(&self, model: &str) -> Option<f64> {
        let (input_ppm, output_ppm) = price_per_million(model)?;
        let cost = (self.input_tokens as f64 * input_ppm + self.output_tokens as f64 * output_ppm)
            / 1_000_000.0;
        Some(cost)
    }
}
