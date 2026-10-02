//! ADR 0045 D3.2 — `catalog_spec` against the golden cases recorded from the TypeScript SDK's
//! own assembly of a catalog spec (`packages/graph-sdk/src/catalog-spec.golden.rust.test.ts`
//! wrote the file and checks the same cases through `@ailu-ai/napi`).

use ailu_runtime_bridge::catalog::catalog_spec_json;
use serde_json::{json, Value};

const GOLDEN: &str = include_str!("fixtures/catalog_spec_golden.json");

/// The engine's answer in the golden shape: `{ spec, warnings }`, or the error's kind (and its
/// node and reason when it has them).
fn engine_result(input: &Value) -> Value {
    let out: Value =
        serde_json::from_str(&catalog_spec_json(&input.to_string()).expect("input parses"))
            .expect("output is JSON");
    let Some(error) = out.get("error") else {
        return out;
    };
    let mut golden = json!({ "kind": error["kind"] });
    for field in ["nodeId", "reason"] {
        if let Some(value) = error.get(field) {
            golden[field] = value.clone();
        }
    }
    json!({ "error": golden })
}

#[test]
fn every_golden_case_gets_the_spec_the_typescript_sdk_built() {
    let cases: Vec<Value> = serde_json::from_str(GOLDEN).expect("the golden file is JSON");
    assert!(cases.len() >= 30, "{} golden cases", cases.len());
    let mut failures = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap_or("?");
        let got = engine_result(&case["input"]);
        if got != case["expected"] {
            failures.push(format!(
                "{name}:\n  expected {}\n  got      {got}",
                case["expected"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
