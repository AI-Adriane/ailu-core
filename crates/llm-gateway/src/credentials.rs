//! Where each provider's API key comes from, and the explicit offline switch.
//!
//! Every engine path (graph agents, prebuilt agents, one-shot `invoke()`) resolves credentials
//! through here, so a provider reads the same variables everywhere, and a missing key fails the
//! same way everywhere instead of silently running on a mock.

use crate::LlmProvider;

/// `AILU_LLM_MOCK=1` runs agents that have no credentials on the deterministic offline mock
/// instead of failing. Meant for tests, CI and trying the examples without an API key.
pub const OFFLINE_MOCK_ENV: &str = "AILU_LLM_MOCK";

/// Whether offline mode is on (`AILU_LLM_MOCK=1`).
pub fn offline_mock_enabled() -> bool {
    std::env::var(OFFLINE_MOCK_ENV).as_deref() == Ok("1")
}

/// The environment variables a provider's API key is read from, in precedence order. Empty for
/// the keyless local servers (Ollama, LM Studio), which a flag enables instead, and for the mock.
pub fn provider_key_env(provider: LlmProvider) -> &'static [&'static str] {
    match provider {
        LlmProvider::Anthropic => &["ANTHROPIC_API_KEY"],
        LlmProvider::Openai => &["OPENAI_API_KEY"],
        LlmProvider::Google => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        LlmProvider::Mistral => &["MISTRAL_API_KEY"],
        LlmProvider::Openrouter => &["OPENROUTER_API_KEY"],
        LlmProvider::Minimax => &["MINIMAX_API_KEY"],
        LlmProvider::Huggingface => &["HF_TOKEN", "HUGGINGFACE_API_KEY"],
        LlmProvider::Ollama | LlmProvider::Lmstudio | LlmProvider::Mock => &[],
    }
}

/// The first non-empty API key for `provider` in the process environment.
pub fn provider_key_from_env(provider: LlmProvider) -> Option<String> {
    provider_key_env(provider)
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

/// The lowercase slug of a provider, as written in `model.<slug>()` and `provider: "<slug>"`.
pub fn provider_slug(provider: LlmProvider) -> &'static str {
    match provider {
        LlmProvider::Openai => "openai",
        LlmProvider::Anthropic => "anthropic",
        LlmProvider::Mistral => "mistral",
        LlmProvider::Ollama => "ollama",
        LlmProvider::Google => "google",
        LlmProvider::Minimax => "minimax",
        LlmProvider::Openrouter => "openrouter",
        LlmProvider::Huggingface => "huggingface",
        LlmProvider::Lmstudio => "lmstudio",
        LlmProvider::Mock => "mock",
    }
}

/// Providers whose key is read from the environment, in the order the error message lists them.
const KEYED_PROVIDERS: [LlmProvider; 7] = [
    LlmProvider::Anthropic,
    LlmProvider::Openai,
    LlmProvider::Google,
    LlmProvider::Mistral,
    LlmProvider::Openrouter,
    LlmProvider::Minimax,
    LlmProvider::Huggingface,
];

/// The error for a provider with no usable credentials. It names exactly what to set, and how to
/// run offline on purpose. `Mock` here means a tier found no provider key at all.
pub fn missing_credentials_message(provider: LlmProvider) -> String {
    let what = match provider {
        LlmProvider::Ollama => {
            "Ollama is not enabled: set AILU_USE_OLLAMA=1 (and AILU_OLLAMA_BASE_URL for a server \
             that is not on localhost)"
                .to_owned()
        }
        LlmProvider::Lmstudio => {
            "LM Studio is not enabled: set AILU_USE_LMSTUDIO=1 (and AILU_LMSTUDIO_BASE_URL for a \
             server that is not on localhost)"
                .to_owned()
        }
        LlmProvider::Mock => {
            let names: Vec<&str> = KEYED_PROVIDERS
                .iter()
                .map(|provider| provider_key_env(*provider)[0])
                .collect();
            format!(
                "no model provider API key found in the environment: set one of {}",
                names.join(", ")
            )
        }
        keyed => format!(
            "no API key for provider '{}': set {}",
            provider_slug(keyed),
            provider_key_env(keyed).join(" or ")
        ),
    };
    format!("{what}. To run offline on the deterministic mock instead, set {OFFLINE_MOCK_ENV}=1.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_message_names_the_variable_and_the_offline_switch() {
        let message = missing_credentials_message(LlmProvider::Anthropic);
        assert!(message.contains("'anthropic'"), "{message}");
        assert!(message.contains("ANTHROPIC_API_KEY"), "{message}");
        assert!(message.contains("AILU_LLM_MOCK=1"), "{message}");
    }

    #[test]
    fn aliases_are_listed_in_precedence_order() {
        assert_eq!(
            provider_key_env(LlmProvider::Huggingface),
            &["HF_TOKEN", "HUGGINGFACE_API_KEY"]
        );
        assert!(missing_credentials_message(LlmProvider::Google)
            .contains("GEMINI_API_KEY or GOOGLE_API_KEY"));
    }

    #[test]
    fn a_tier_with_no_key_lists_every_provider_key() {
        let message = missing_credentials_message(LlmProvider::Mock);
        for name in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "HF_TOKEN",
        ] {
            assert!(message.contains(name), "{message}");
        }
    }

    #[test]
    fn local_servers_name_their_enable_flag() {
        assert!(missing_credentials_message(LlmProvider::Ollama).contains("AILU_USE_OLLAMA=1"));
        assert!(missing_credentials_message(LlmProvider::Lmstudio).contains("AILU_USE_LMSTUDIO=1"));
    }
}
