//! ADR 0051 D1 — the canonical form a call key hashes, against the shared reference vectors that
//! TypeScript (`callKeyOf`, napi) and Python (`ailu.call_key_of`) are checked against too. The form
//! is frozen: it is what every content-scoped (ADR 0024) and conditioned (ADR 0046/0048) grant has
//! hashed since it shipped, so no key filed or granted before ADR 0051 changes.

use ailu_agents_core::tools::approval_key;
use ailu_agents_core::{call_input_of, call_input_of_json, call_key_of, call_key_of_json};
use serde_json::Value;

const VECTORS: &str = include_str!("fixtures/call_key_vectors.json");

fn vectors() -> Vec<Value> {
    let file: Value = serde_json::from_str(VECTORS).expect("the vectors are JSON");
    file["vectors"].as_array().expect("a vector list").clone()
}

fn text<'a>(vector: &'a Value, field: &str) -> &'a str {
    vector[field].as_str().expect("a string field")
}

#[test]
fn every_vector_hashes_to_its_call_key() {
    let vectors = vectors();
    assert!(vectors.len() >= 20, "{} vectors", vectors.len());
    let mut failures = Vec::new();
    for vector in &vectors {
        let (name, input_json) = (text(vector, "name"), text(vector, "inputJson"));
        let input: Value = serde_json::from_str(input_json).expect("the input parses");
        let got = (
            call_input_of(&input),
            call_key_of(name, &input),
            call_input_of_json(input_json).expect("the input parses"),
            call_key_of_json(name, input_json).expect("the input parses"),
        );
        let want = (
            text(vector, "callInput").to_owned(),
            text(vector, "callKey").to_owned(),
            text(vector, "callInput").to_owned(),
            text(vector, "callKey").to_owned(),
        );
        if got != want {
            failures.push(format!(
                "{}:\n  expected {want:?}\n  got      {got:?}",
                text(vector, "description")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_call_key_is_the_grant_key_of_a_call_and_the_form_is_idempotent() {
    for vector in vectors() {
        let (name, input_json) = (text(&vector, "name"), text(&vector, "inputJson"));
        let input: Value = serde_json::from_str(input_json).expect("the input parses");
        // The key a content-scoped or conditioned grant has always used.
        assert_eq!(call_key_of(name, &input), approval_key(name, true, &input));
        // A host that kept `callInput` gets the same key back from it.
        let call_input = text(&vector, "callInput");
        assert_eq!(
            call_key_of_json(name, call_input).as_deref(),
            Ok(text(&vector, "callKey"))
        );
        assert_eq!(call_input_of_json(call_input).as_deref(), Ok(call_input));
    }
}

#[test]
fn input_that_is_not_json_is_refused() {
    assert!(call_key_of_json("refund", "{").is_err());
    assert!(call_input_of_json("not json").is_err());
}
