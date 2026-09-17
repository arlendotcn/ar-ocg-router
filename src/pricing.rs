//! Token price tables (USD per 1M tokens) used for local cost accounting / savings.
//!
//! DeepSeek official (https://api-docs.deepseek.com/quick_start/pricing/) and OpenCode Go use
//! the *same* numbers for DeepSeek models, which is why OpenCode Go quota is consumed at exactly
//! the cash price of the same request.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cached_input: f64,
}

impl Prices {
    pub const fn new(input: f64, output: f64, cached_input: f64) -> Prices {
        Prices {
            input,
            output,
            cached_input,
        }
    }

    pub fn scale(&self, factor: f64) -> Prices {
        Prices {
            input: self.input * factor,
            output: self.output * factor,
            cached_input: self.cached_input * factor,
        }
    }
}

fn family_prices(family: &str) -> Option<Prices> {
    match family {
        "deepseek-flash" | "deepseek-v4.1-flash" | "deepseek-v4-flash"
        | "deepseek-v4-flash-vision-exp" => Some(Prices::new(0.15, 0.60, 0.003)),
        "deepseek-v4-pro" => Some(Prices::new(0.66, 1.98, 0.022)),
        // Common OpenCode Go catalog entries (off-peak/base rates).
        "glm-5.3-flash" => Some(Prices::new(0.075, 0.25, 0.015)),
        "glm-5.3" | "glm-5.2" | "glm-5.1" => Some(Prices::new(1.40, 4.40, 0.26)),
        "kimi-k3" => Some(Prices::new(3.00, 15.00, 0.30)),
        "kimi-k2.7-code" => Some(Prices::new(0.95, 4.00, 0.19)),
        "kimi-k2.6" => Some(Prices::new(0.95, 4.00, 0.16)),
        "minimax-m3" | "minimax-m2.7" | "minimax-m2.5" => Some(Prices::new(0.30, 1.20, 0.06)),
        "mimo-v2.5" => Some(Prices::new(0.14, 0.28, 0.0028)),
        "mimo-v2.5-pro" => Some(Prices::new(0.435, 0.87, 0.003625)),
        "qwen3.8-max" => Some(Prices::new(2.00, 6.00, 0.25)),
        "qwen3.8-flash" => Some(Prices::new(0.15, 0.47, 0.016)),
        "qwen3.7-max" => Some(Prices::new(2.50, 7.50, 0.50)),
        "qwen3.7-plus" => Some(Prices::new(0.40, 1.60, 0.04)),
        "qwen3.6-plus" => Some(Prices::new(0.50, 3.00, 0.05)),
        "hy3" => Some(Prices::new(0.14, 0.58, 0.035)),
        "gpt-5.6-luna" => Some(Prices::new(0.20, 1.20, 0.02)),
        "grok-4.6" => Some(Prices::new(2.00, 6.00, 0.50)),
        _ => None,
    }
}

/// DeepSeek-family prices. Peak = 01:00-04:00 / 06:00-10:00 UTC Mon-Fri; off-peak is half price.
/// OpenCode Go burns quota at the same rates, so this is both the cash price of the DeepSeek
/// official API and the quota consumption of an equivalent OpenCode Go request.
pub fn deepseek_prices(family: &str, peak: bool) -> Option<Prices> {
    let base = family_prices(family)?;
    Some(if peak { base.scale(2.0) } else { base })
}

/// Prices for any catalog model; the half-price off-peak rule only applies to DeepSeek models.
pub fn prices_for(family: &str, peak: bool) -> Option<Prices> {
    let base = family_prices(family)?;
    if family.starts_with("deepseek") {
        // The table holds off-peak rates; peak is exactly double (verified against
        // api-docs.deepseek.com and opencode.ai/docs/go).
        Some(if peak { base.scale(2.0) } else { base })
    } else {
        Some(base)
    }
}

pub fn cost_of(p: &Prices, prompt_tokens: u64, cached_tokens: u64, completion_tokens: u64) -> f64 {
    let cached = cached_tokens.min(prompt_tokens);
    let miss = prompt_tokens.saturating_sub(cached);
    (miss as f64 * p.input + cached as f64 * p.cached_input + completion_tokens as f64 * p.output)
        / 1_000_000.0
}
