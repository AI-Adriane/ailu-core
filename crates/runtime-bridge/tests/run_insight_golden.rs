//! ADR 0045 D3.4 — `explain_run` and `verify_replay_decisions` against the golden cases recorded
//! from the TypeScript SDK's `explainRun` / `verifyReplayDecisions`
//! (`packages/graph-sdk/src/run-insight.golden.rust.test.ts` wrote the file).

use ailu_runtime_bridge::run_insight::{explain_run, verify_replay_decisions};
use serde_json::Value;

const GOLDEN: &str = include_str!("fixtures/run_insight_golden.json");

fn list(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

#[test]
fn every_golden_case_gets_the_answer_the_typescript_sdk_gave() {
    let cases: Vec<Value> = serde_json::from_str(GOLDEN).expect("the golden file is JSON");
    assert!(cases.len() >= 20, "{} golden cases", cases.len());
    let mut failures = Vec::new();
    for case in &cases {
        let input = &case["input"];
        let got = match case["kind"].as_str() {
            Some("explain") => {
                let events = input.get("events").map(list);
                explain_run(&input["state"], events.as_deref())
            }
            Some("verify") => {
                verify_replay_decisions(&list(&input["attested"]), &list(&input["replayed"]))
            }
            other => panic!("unknown golden kind {other:?}"),
        };
        if got != case["expected"] {
            failures.push(format!(
                "{}:\n  expected {}\n  got      {got}",
                case["name"], case["expected"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
