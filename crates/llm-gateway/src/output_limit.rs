//! Output budget for generation (ADR-600, product decision 239).

pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

/// Apply the budget after resolving the provider's model ID. Unknown models retain
/// the requested budget; their provider enforces any additional model/context limit.
/// Sources and the bounded compatibility table are recorded in ADR 0060.
pub(crate) fn output_limit(model: &str, requested: Option<u32>) -> u32 {
    let model = model
        .strip_prefix("openai/")
        .or_else(|| model.strip_prefix("anthropic/"))
        .or_else(|| model.strip_prefix("google/"))
        .unwrap_or(model);
    let cap = match model {
        "gpt-4" | "gpt-4-0613" => 8192,
        "claude-3-haiku-20240307" | "gpt-4-turbo" | "gpt-3.5-turbo" => 4096,
        "gemini-2.0-flash" | "gemini-2.0-flash-001" => 8192,
        _ => u32::MAX,
    };
    requested.unwrap_or(DEFAULT_MAX_TOKENS).min(cap)
}
