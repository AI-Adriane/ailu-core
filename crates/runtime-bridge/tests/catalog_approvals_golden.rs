//! ADR 0045 D3.1 — the engine's approval decisions against the golden cases recorded from the
//! TypeScript SDK's own (`packages/graph-sdk/src/catalog-approvals.golden.rust.test.ts` wrote the
//! file): what a catalog run files, and whether a resume may go on.

use ailu_runtime_bridge::catalog_approvals::{
    approvals_to_check, filing_plan, resume_problems, FilingInput, ResumeCheckInput,
    APPROVAL_IDS_CHANNEL,
};
use serde_json::{json, Value};

const GOLDEN: &str = include_str!("fixtures/catalog_approvals_golden.json");

/// What the host keeps in `__approvalIds` after following the plan, with the golden recorder's
/// ids (`filed-<n>`); `null` when the channel is absent.
fn kept_ids(state: &Value, clear: bool, filed: usize) -> Value {
    if filed > 0 {
        return json!((0..filed).map(|n| format!("filed-{n}")).collect::<Vec<_>>());
    }
    if clear {
        return json!([]);
    }
    state["channels"]
        .get(APPROVAL_IDS_CHANNEL)
        .cloned()
        .unwrap_or(Value::Null)
}

fn engine_result(case: &Value) -> Value {
    match case["kind"].as_str() {
        Some("filing") => {
            let input: FilingInput =
                serde_json::from_value(case["input"].clone()).expect("filing input parses");
            let plan = filing_plan(&input);
            let mut result = json!({
                "requests": plan.requests,
                "approvalIds": kept_ids(&input.state, plan.clear_approval_ids, plan.requests.len()),
            });
            // ADR 0045 rev. 1 R6: a refused plan says why; every other case keeps its shape.
            if let Some(refusal) = plan.refusal {
                result["refusal"] = json!(refusal);
            }
            result
        }
        Some("check") => {
            let input: ResumeCheckInput =
                serde_json::from_value(case["input"].clone()).expect("check input parses");
            json!({
                "reads": approvals_to_check(&input.state),
                "problems": resume_problems(&input),
            })
        }
        other => panic!("unknown golden kind {other:?}"),
    }
}

#[test]
fn every_golden_case_gets_the_decision_the_typescript_sdk_made() {
    let cases: Vec<Value> = serde_json::from_str(GOLDEN).expect("the golden file is JSON");
    assert!(cases.len() >= 30, "{} golden cases", cases.len());
    let mut failures = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap_or("?");
        let got = engine_result(case);
        if got != case["expected"] {
            failures.push(format!(
                "{name}:\n  expected {}\n  got      {got}",
                case["expected"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
