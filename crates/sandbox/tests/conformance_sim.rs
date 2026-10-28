//! The conformance suite run against the simulated backend, from outside the crate: it uses
//! the public API only, as any backend's own test would.

use ailu_sandbox::conformance::{catalogue, run_conformance, ConformanceReport, SimTarget};

/// What a failing run prints: the id and the reason of each failed case.
fn failures(report: &ConformanceReport) -> Vec<String> {
    report
        .failures()
        .map(|result| format!("{}: {:?}", result.case.id, result.outcome))
        .collect()
}

#[tokio::test]
async fn the_simulator_passes_every_case_that_applies_to_it() {
    let report = run_conformance(&SimTarget::new()).await;
    assert!(
        report.is_conformant(),
        "failed cases: {:#?}",
        failures(&report)
    );
    assert_eq!(report.cases.len(), catalogue().len());
}
