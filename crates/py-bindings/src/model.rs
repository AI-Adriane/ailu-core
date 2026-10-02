//! One-shot model calls for Python (ADR 0045 M4): the gateway call behind the TypeScript
//! `Model.invoke()`, run to completion on a current-thread tokio runtime on the calling thread
//! (the pyo3 layer releases the GIL around it).

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
