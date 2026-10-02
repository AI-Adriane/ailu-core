//! One-shot model calls for Python (ADR 0045 M4): the gateway call behind the TypeScript
//! `Model.invoke()`, and the embeddings call behind `createEmbeddings`, each run to completion on a
//! current-thread tokio runtime on the calling thread (the pyo3 layer releases the GIL around it).

/// Complete a serialized `LlmRequest` (an optional `baseUrl` targets a custom OpenAI-compatible
/// endpoint) with `provider_keys_json` (`{ "<provider>": "<key>" }`, `"{}"` for the env keys).
/// Returns the serialized `LlmResponse`.
///
/// # Errors
///
/// Invalid JSON, a provider without credentials (outside offline mode), or the provider's error.
pub fn llm_complete(request_json: &str, provider_keys_json: &str) -> Result<String, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the engine runtime: {error}"))?
        .block_on(ailu_runtime_bridge::llm_complete_json(
            request_json,
            provider_keys_json,
        ))
}

/// Embed texts through the provider's API (`{ provider?, apiKey?, model?, baseUrl?, dimensions? }`
/// and a JSON array of texts); returns the vectors as JSON.
///
/// # Errors
///
/// Invalid JSON, a missing key, a failed request or a malformed response.
pub fn embed(options_json: &str, texts_json: &str) -> Result<String, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not start the engine runtime: {error}"))?
        .block_on(ailu_runtime_bridge::vectors::embed_json(
            options_json,
            texts_json,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_provider_answers_offline() {
        let response = llm_complete(
            r#"{"provider":"mock","model":"","messages":[{"role":"user","content":"hi"}]}"#,
            "{}",
        )
        .expect("the mock answers");
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["provider"], "mock");
        assert!(response["content"]
            .as_str()
            .is_some_and(|text| !text.is_empty()));
    }

    #[test]
    fn a_request_that_does_not_parse_is_an_error() {
        let error = llm_complete("{", "{}").expect_err("invalid request");
        assert!(error.contains("invalid LLM request JSON"), "{error}");
        let error = llm_complete(r#"{"provider":"nope","model":"","messages":[]}"#, "{}")
            .expect_err("unknown provider");
        assert!(error.contains("invalid LLM request JSON"), "{error}");
    }
}
