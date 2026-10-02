//! Text embeddings from a provider's `/embeddings` API (ADR 0045 M4): the engine's side of the
//! TypeScript `createEmbeddings`, which the Python SDK calls.
//!
//! Mistral and OpenAI share the wire: POST `{ model, input, dimensions? }` to `{base}/embeddings`
//! with `Authorization: Bearer <key>`, and read `data[].embedding` in order. The request body, the
//! defaults and the reading of a response are those of the TypeScript helper (checked against
//! golden cases recorded from it); errors are worded without its function name.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::http::http_client;

/// The providers whose embeddings API this module reaches directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingsProvider {
    #[default]
    Mistral,
    Openai,
}

impl EmbeddingsProvider {
    /// The provider's slug.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            EmbeddingsProvider::Mistral => "mistral",
            EmbeddingsProvider::Openai => "openai",
        }
    }

    /// The model used when none is given.
    #[must_use]
    pub fn default_model(self) -> &'static str {
        match self {
            EmbeddingsProvider::Mistral => "mistral-embed",
            EmbeddingsProvider::Openai => "text-embedding-3-small",
        }
    }

    /// The API base URL used when none is given.
    #[must_use]
    pub fn default_base_url(self) -> &'static str {
        match self {
            EmbeddingsProvider::Mistral => "https://api.mistral.ai/v1",
            EmbeddingsProvider::Openai => "https://api.openai.com/v1",
        }
    }

    /// The environment variable holding the provider's key.
    #[must_use]
    pub fn key_env(self) -> &'static str {
        match self {
            EmbeddingsProvider::Mistral => "MISTRAL_API_KEY",
            EmbeddingsProvider::Openai => "OPENAI_API_KEY",
        }
    }
}

/// How to reach the embeddings API; every field but the provider is optional.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingsOptions {
    #[serde(default)]
    pub provider: EmbeddingsProvider,
    /// The key; else the provider's environment variable.
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Down-project the vectors (OpenAI `text-embedding-3-*`); sent only when set.
    #[serde(default)]
    pub dimensions: Option<u32>,
}

/// Why texts got no vectors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EmbeddingsError {
    #[error("no API key for the {provider} embeddings API: pass one or set {env}")]
    MissingKey {
        provider: &'static str,
        env: &'static str,
    },
    #[error("malformed embeddings response: {0}")]
    Response(String),
    #[error("embeddings request failed: {0}")]
    Transport(String),
}

/// The request body for `texts`: `{ model, input, dimensions? }`.
#[must_use]
pub fn embeddings_body(options: &EmbeddingsOptions, texts: &[String]) -> Value {
    let model = options
        .model
        .clone()
        .unwrap_or_else(|| options.provider.default_model().to_owned());
    let mut body = json!({ "model": model, "input": texts });
    if let Some(dimensions) = options.dimensions {
        body["dimensions"] = json!(dimensions);
    }
    body
}

/// Read `{ data: [{ embedding: number[] }, …] }` into the vectors, in order.
///
/// # Errors
///
/// [`EmbeddingsError::Response`] when the response is not an object, `data` is not a list, or an
/// entry's `embedding` is not a list of numbers.
pub fn parse_embeddings_response(response: &Value) -> Result<Vec<Vec<f64>>, EmbeddingsError> {
    let object = response
        .as_object()
        .ok_or_else(|| EmbeddingsError::Response("response is not an object".to_owned()))?;
    let data = object
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| EmbeddingsError::Response("`data` is not an array".to_owned()))?;
    data.iter()
        .enumerate()
        .map(|(index, entry)| {
            entry
                .get("embedding")
                .and_then(Value::as_array)
                .and_then(|numbers| {
                    numbers
                        .iter()
                        .map(Value::as_f64)
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| {
                    EmbeddingsError::Response(format!(
                        "`data[{index}].embedding` is not a number[]"
                    ))
                })
        })
        .collect()
}

/// Embed `texts` through the provider's API. No texts: no call, no vectors.
///
/// # Errors
///
/// A missing key, a failed or non-2xx request, or a response [`parse_embeddings_response`]
/// refuses.
pub async fn embed(
    options: &EmbeddingsOptions,
    texts: &[String],
) -> Result<Vec<Vec<f64>>, EmbeddingsError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let provider = options.provider;
    // As in TypeScript (`apiKey ?? env`): an empty key given is missing, not a reason to read
    // the environment.
    let key = options
        .api_key
        .clone()
        .or_else(|| std::env::var(provider.key_env()).ok())
        .filter(|key| !key.is_empty())
        .ok_or(EmbeddingsError::MissingKey {
            provider: provider.slug(),
            env: provider.key_env(),
        })?;
    let base_url = options
        .base_url
        .clone()
        .unwrap_or_else(|| provider.default_base_url().to_owned());
    let response = http_client()
        .post(format!("{base_url}/embeddings"))
        .bearer_auth(key)
        .json(&embeddings_body(options, texts))
        .send()
        .await
        .map_err(|error| EmbeddingsError::Transport(error.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| EmbeddingsError::Transport(error.to_string()))?;
    if !status.is_success() {
        return Err(EmbeddingsError::Response(format!(
            "status {}: {text}",
            status.as_u16()
        )));
    }
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|error| EmbeddingsError::Response(format!("not JSON: {error}")))?;
    parse_embeddings_response(&parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_takes_the_provider_defaults_and_the_dimensions_only_when_set() {
        let texts = vec!["a".to_owned()];
        assert_eq!(
            embeddings_body(&EmbeddingsOptions::default(), &texts),
            json!({ "model": "mistral-embed", "input": ["a"] })
        );
        let openai = EmbeddingsOptions {
            provider: EmbeddingsProvider::Openai,
            dimensions: Some(256),
            ..EmbeddingsOptions::default()
        };
        assert_eq!(
            embeddings_body(&openai, &texts),
            json!({ "model": "text-embedding-3-small", "input": ["a"], "dimensions": 256 })
        );
    }

    #[test]
    fn a_response_reads_in_order_or_says_what_is_wrong() {
        let vectors = parse_embeddings_response(&json!({
            "data": [{ "embedding": [0.5, 1] }, { "embedding": [] }]
        }))
        .unwrap();
        assert_eq!(vectors, vec![vec![0.5, 1.0], vec![]]);
        let wrong =
            parse_embeddings_response(&json!({ "data": [{ "embedding": [1, "2"] }] })).unwrap_err();
        assert_eq!(
            wrong.to_string(),
            "malformed embeddings response: `data[0].embedding` is not a number[]"
        );
    }

    #[tokio::test]
    async fn no_texts_make_no_call_and_a_missing_key_names_the_variable() {
        assert_eq!(
            embed(&EmbeddingsOptions::default(), &[]).await.unwrap(),
            Vec::<Vec<f64>>::new()
        );
        // An empty key is missing whatever the environment holds, as in TypeScript.
        let options = EmbeddingsOptions {
            provider: EmbeddingsProvider::Openai,
            api_key: Some(String::new()),
            base_url: Some("http://127.0.0.1:1".to_owned()),
            ..EmbeddingsOptions::default()
        };
        let error = embed(&options, &["x".to_owned()]).await.unwrap_err();
        assert_eq!(
            error,
            EmbeddingsError::MissingKey {
                provider: "openai",
                env: "OPENAI_API_KEY"
            }
        );
        assert_eq!(
            error.to_string(),
            "no API key for the openai embeddings API: pass one or set OPENAI_API_KEY"
        );
    }

    #[tokio::test]
    async fn a_call_posts_the_body_with_the_key_and_reads_the_vectors() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            // Read until the JSON body has arrived (headers, then `{ … }`).
            while !String::from_utf8_lossy(&request).contains("\"input\"") {
                let read = stream.read(&mut buffer).expect("read");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let body = r#"{"data":[{"embedding":[0.25,-1]}]}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            String::from_utf8_lossy(&request).into_owned()
        });
        let options = EmbeddingsOptions {
            api_key: Some("sk-test".to_owned()),
            base_url: Some(format!("http://{address}/v1")),
            ..EmbeddingsOptions::default()
        };
        let vectors = embed(&options, &["hi".to_owned()]).await.expect("embeds");
        assert_eq!(vectors, vec![vec![0.25, -1.0]]);
        let request = server.join().expect("server");
        assert!(request.starts_with("POST /v1/embeddings"), "{request}");
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer sk-test"),
            "{request}"
        );
        assert!(request.contains(r#""model":"mistral-embed""#), "{request}");
    }
}
