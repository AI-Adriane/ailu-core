//! Defects of the guard itself, in the proof handling. They cannot be built from outside the
//! crate, so the guard has a switch that exists only in tests; each defect must be caught by the
//! cases of the `proof` group.

use super::{run_conformance, CaseOutcome, ConformanceReport, SimTarget};
use crate::guard::{ProofFault, NEXT_FAULT};

/// Makes the guards built on this thread carry `fault`, and takes it back when dropped.
struct Faulty;

impl Faulty {
    fn with(fault: ProofFault) -> Self {
        NEXT_FAULT.with(|slot| slot.set(Some(fault)));
        Self
    }
}

impl Drop for Faulty {
    fn drop(&mut self) {
        NEXT_FAULT.with(|slot| slot.set(None));
    }
}

fn failed(report: &ConformanceReport) -> Vec<&'static str> {
    report
        .cases
        .iter()
        .filter(|result| matches!(result.outcome, CaseOutcome::Failed(_)))
        .map(|result| result.case.id)
        .collect()
}

async fn assert_caught(fault: ProofFault, expected: &[&str]) {
    let faulty = Faulty::with(fault);
    let report = run_conformance(&SimTarget::new()).await;
    drop(faulty);
    let caught = failed(&report);
    assert!(!report.is_conformant(), "{fault:?} was not detected");
    for id in expected {
        assert!(
            caught.contains(id),
            "{fault:?} was not caught by {id}; failed: {caught:?}"
        );
    }
    // Only the proof group notices: the rest of the contract is untouched by these defects.
    assert!(
        caught.iter().all(|id| id.starts_with("proof.")),
        "{fault:?} failed cases outside the proof group: {caught:?}"
    );
}

#[tokio::test]
async fn a_guard_without_the_defect_passes_every_case() {
    let report = run_conformance(&SimTarget::new()).await;
    assert!(report.is_conformant(), "failed: {:?}", failed(&report));
}

#[tokio::test]
async fn a_result_recorded_before_it_is_signed_is_served_unsigned_on_replay() {
    assert_caught(
        ProofFault::SettleBeforeSign,
        &[
            "proof.no_result_without_signature_on_replay",
            "proof.signer_unavailable_fails_closed",
        ],
    )
    .await;
}

#[tokio::test]
async fn a_signer_that_fails_is_ignored_and_the_result_goes_out() {
    assert_caught(
        ProofFault::IgnoreSignerError,
        &[
            "proof.signer_unavailable_fails_closed",
            "proof.signer_refusal_fails_closed",
            "proof.no_result_without_signature_on_replay",
        ],
    )
    .await;
}

#[tokio::test]
async fn a_record_that_is_not_about_the_submitted_proof_is_accepted() {
    assert_caught(
        ProofFault::SkipReturnedRecordCheck,
        &["proof.record_must_match_the_submission"],
    )
    .await;
}
