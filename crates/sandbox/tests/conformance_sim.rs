//! The conformance suite run against the simulated backend, from outside the crate: it uses
//! the public API only, as any backend's own test would.

use ailu_sandbox::conformance::{catalogue, run_conformance, ConformanceReport, SimTarget};

/// The simulator passes at least this many cases; raised as the suite grows.
const MIN_PASSED: usize = 32;

/// The cases the simulator cannot take part in, with the reason. None so far: it offers every
/// probe and every hook.
const NOT_APPLICABLE: &[&str] = &[];

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
    let skipped: Vec<_> = report
        .not_applicable_cases()
        .map(|result| result.case.id)
        .collect();
    assert_eq!(skipped, NOT_APPLICABLE, "cases the simulator skipped");
    assert!(
        report.passed() >= MIN_PASSED,
        "only {} cases passed out of {}",
        report.passed(),
        report.cases.len()
    );
}
