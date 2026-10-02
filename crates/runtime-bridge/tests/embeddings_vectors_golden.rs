//! ADR 0045 M4 — the engine's embeddings and vector search against the golden cases recorded from
//! the TypeScript `createEmbeddings`, `createVectorStore().query` and `cosineSimilarity`
//! (`packages/graph-sdk/src/embeddings-vectors.golden.test.ts` wrote the file).

use ailu_llm_gateway::embeddings::{embeddings_body, parse_embeddings_response, EmbeddingsOptions};
use ailu_runtime_bridge::vectors::{cosine_similarity, query_vectors};
use serde_json::Value;

const GOLDEN: &str = include_str!("fixtures/embeddings_vectors_golden.json");

fn floats(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// Two numbers read from JSON equal: `serde_json`'s default float parsing can be one unit in the
/// last place off for a 16-17 digit decimal (measured: `0.9938837346736189` reads as
/// `…188`), so the golden value read here may sit one ULP from the exact one the engine computes.
fn same_number(x: f64, y: f64) -> bool {
    x == y || (x - y).abs() <= 2.0 * f64::EPSILON * x.abs().max(y.abs())
}

/// JSON values equal, numbers compared as `f64` (TypeScript writes `1`, Rust `1.0`).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => same_number(x, y),
            _ => false,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, x)| y.get(key).is_some_and(|y| same(x, y)))
        }
        _ => a == b,
    }
}

#[test]
fn the_engine_embeds_reads_queries_and_scores_as_the_typescript_sdk_did() {
    let golden: Value = serde_json::from_str(GOLDEN).expect("the golden file is JSON");
    let mut failures = Vec::new();

    for case in golden["bodies"].as_array().expect("bodies") {
        let options: EmbeddingsOptions =
            serde_json::from_value(case["options"].clone()).expect("options");
        let texts: Vec<String> = serde_json::from_value(case["texts"].clone()).expect("texts");
        // No texts: the SDK made no call, so there is no body to compare.
        let got = if texts.is_empty() {
            Value::Null
        } else {
            embeddings_body(&options, &texts)
        };
        if !same(&got, &case["expected"]) {
            failures.push(format!("body {}: {got}", case["name"]));
        }
    }

    for case in golden["responses"].as_array().expect("responses") {
        let got = match parse_embeddings_response(&case["response"]) {
            Ok(vectors) => serde_json::json!({ "vectors": vectors }),
            // The TypeScript message starts with its function's name.
            Err(error) => serde_json::json!({ "error": format!("createEmbeddings: {error}") }),
        };
        if !same(&got, &case["expected"]) {
            failures.push(format!("response {}: {got}", case["name"]));
        }
    }

    for case in golden["queries"].as_array().expect("queries") {
        // The store keeps one item per id, in first-insertion order, the last write winning.
        let mut store: Vec<Value> = Vec::new();
        for item in case["items"].as_array().expect("items") {
            match store.iter_mut().find(|stored| stored["id"] == item["id"]) {
                Some(stored) => *stored = item.clone(),
                None => store.push(item.clone()),
            }
        }
        let found = query_vectors(
            &store,
            &floats(&case["embedding"]),
            case["k"].as_i64().expect("k"),
        );
        let got = serde_json::json!({ "size": store.len(), "matches": found });
        if !same(&got, &case["expected"]) {
            failures.push(format!("query {}: {got}", case["name"]));
        }
    }

    for case in golden["cosines"].as_array().expect("cosines") {
        let got = cosine_similarity(&floats(&case["a"]), &floats(&case["b"]));
        if !case["expected"]
            .as_f64()
            .is_some_and(|expected| same_number(got, expected))
        {
            failures.push(format!("cosine {}: {got}", case["name"]));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
