//! Embeddings and nearest-neighbour search for the SDKs (ADR 0045 M4): the engine's side of the
//! TypeScript `createEmbeddings`, `createVectorStore().query` and `cosineSimilarity`, which the
//! Python SDK calls. Checked against golden cases recorded from the TypeScript helpers.

use ailu_llm_gateway::embeddings::{embed, parse_embeddings_response, EmbeddingsOptions};
use serde_json::{json, Map, Value};

/// Cosine similarity of two vectors, as the TypeScript `cosineSimilarity`: the dot product over
/// the common prefix, each norm over the whole vector, `0` when either norm is zero.
#[must_use]
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    let common = a.len().min(b.len());
    let (mut dot, mut norm_a, mut norm_b) = (0.0_f64, 0.0_f64, 0.0_f64);
    for i in 0..common {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    for value in &a[common..] {
        norm_a += value * value;
    }
    for value in &b[common..] {
        norm_b += value * value;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    // `+ 0.0` turns a negative zero into zero: JavaScript serializes both as `0`.
    dot / (norm_a.sqrt() * norm_b.sqrt()) + 0.0
}

fn numbers(value: Option<&Value>) -> Vec<f64> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// The `k` items (`{ id, content, embedding, metadata? }`, in store order) most similar to
/// `embedding`: `{ id, content, score, metadata? }`, highest score first, store order breaking
/// ties; none for `k <= 0`.
#[must_use]
pub fn query_vectors(items: &[Value], embedding: &[f64], k: i64) -> Vec<Value> {
    let mut scored: Vec<(usize, f64, &Value)> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            (
                index,
                cosine_similarity(embedding, &numbers(item.get("embedding"))),
                item,
            )
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    let take = usize::try_from(k.max(0)).unwrap_or(0);
    scored
        .into_iter()
        .take(take)
        .map(|(_, score, item)| {
            let mut found = Map::new();
            found.insert(
                "id".to_owned(),
                item.get("id").cloned().unwrap_or(Value::Null),
            );
            found.insert(
                "content".to_owned(),
                item.get("content").cloned().unwrap_or(Value::Null),
            );
            found.insert("score".to_owned(), json!(score));
            if let Some(metadata) = item.get("metadata") {
                found.insert("metadata".to_owned(), metadata.clone());
            }
            Value::Object(found)
        })
        .collect()
}

/// [`query_vectors`] for the bindings: the items and the query embedding as JSON arrays in, the
/// matches as a JSON array out.
///
/// # Errors
///
/// When either input is not a JSON array.
pub fn query_vectors_json(
    items_json: &str,
    embedding_json: &str,
    k: i64,
) -> Result<String, String> {
    let items: Vec<Value> = serde_json::from_str(items_json)
        .map_err(|error| format!("invalid vector items JSON: {error}"))?;
    let embedding: Vec<f64> = serde_json::from_str(embedding_json)
        .map_err(|error| format!("invalid query embedding JSON: {error}"))?;
    Ok(Value::Array(query_vectors(&items, &embedding, k)).to_string())
}

/// [`cosine_similarity`] for the bindings: two JSON arrays of numbers in.
///
/// # Errors
///
/// When either input is not a JSON array of numbers.
pub fn cosine_similarity_json(a_json: &str, b_json: &str) -> Result<f64, String> {
    let a: Vec<f64> =
        serde_json::from_str(a_json).map_err(|error| format!("invalid vector JSON: {error}"))?;
    let b: Vec<f64> =
        serde_json::from_str(b_json).map_err(|error| format!("invalid vector JSON: {error}"))?;
    Ok(cosine_similarity(&a, &b))
}

/// Embed texts through the provider's API, for the bindings: `{ provider?, apiKey?, model?,
/// baseUrl?, dimensions? }` and a JSON array of texts in, the vectors as JSON out.
///
/// # Errors
///
/// Invalid JSON, a missing key, a failed request or a malformed response.
pub async fn embed_json(options_json: &str, texts_json: &str) -> Result<String, String> {
    let options: EmbeddingsOptions = serde_json::from_str(options_json)
        .map_err(|error| format!("invalid embeddings options JSON: {error}"))?;
    let texts: Vec<String> =
        serde_json::from_str(texts_json).map_err(|error| format!("invalid texts JSON: {error}"))?;
    let vectors = embed(&options, &texts)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!(vectors).to_string())
}

/// The request body (JSON) an embeddings call sends for `texts` (`{ model, input, dimensions? }`),
/// for an SDK that makes the call through its own transport.
///
/// # Errors
///
/// Invalid options or texts JSON.
pub fn embeddings_body_json(options_json: &str, texts_json: &str) -> Result<String, String> {
    let options: EmbeddingsOptions = serde_json::from_str(options_json)
        .map_err(|error| format!("invalid embeddings options JSON: {error}"))?;
    let texts: Vec<String> =
        serde_json::from_str(texts_json).map_err(|error| format!("invalid texts JSON: {error}"))?;
    Ok(ailu_llm_gateway::embeddings::embeddings_body(&options, &texts).to_string())
}

/// Read an embeddings API response (JSON) into its vectors (JSON), for an SDK whose own
/// transport made the call.
///
/// # Errors
///
/// The response is not JSON, or not `{ data: [{ embedding: number[] }] }`.
pub fn parse_embeddings_response_json(response_json: &str) -> Result<String, String> {
    let response: Value = serde_json::from_str(response_json)
        .map_err(|error| format!("invalid embeddings response JSON: {error}"))?;
    parse_embeddings_response(&response)
        .map(|vectors| json!(vectors).to_string())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_matches_javascript_on_different_lengths_and_zero_vectors() {
        assert!(
            (cosine_similarity(&[1.0, 2.0], &[1.0, 2.0, 3.0]) - 0.597_614_304_667_196_8).abs()
                < 1e-15
        );
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        // A negative zero reads as zero, as JavaScript writes it.
        assert!(cosine_similarity(&[-1.0], &[0.0, 1.0]).is_sign_positive());
    }

    #[test]
    fn a_query_ranks_by_score_then_store_order_and_carries_metadata() {
        let items = vec![
            json!({ "id": "a", "content": "A", "embedding": [1, 0] }),
            json!({ "id": "b", "content": "B", "embedding": [1, 0], "metadata": { "page": 2 } }),
            json!({ "id": "c", "content": "C", "embedding": [0, 1] }),
        ];
        let found = query_vectors(&items, &[1.0, 0.0], 2);
        assert_eq!(
            found,
            vec![
                json!({ "id": "a", "content": "A", "score": 1.0 }),
                json!({ "id": "b", "content": "B", "score": 1.0, "metadata": { "page": 2 } }),
            ]
        );
        assert!(query_vectors(&items, &[1.0, 0.0], -1).is_empty());
    }

    #[test]
    fn the_json_entries_refuse_input_that_does_not_parse() {
        assert!(query_vectors_json("{}", "[1]", 1).is_err());
        assert!(cosine_similarity_json("[1]", "x").is_err());
        assert!(parse_embeddings_response_json("{").is_err());
        assert_eq!(
            parse_embeddings_response_json(r#"{"data":[{"embedding":[1]}]}"#).unwrap(),
            "[[1.0]]"
        );
    }
}
