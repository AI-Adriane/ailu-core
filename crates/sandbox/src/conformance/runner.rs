//! Running the cases against a target.

use super::cases::{self, CaseDef};
use super::report::{Case, CaseOutcome, CaseResult, ConformanceReport};
use super::support::{Lab, Source, Stop, Verdict};
use super::target::ConformanceTarget;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Longest reason kept in a report.
const MAX_REASON_CHARS: usize = 240;

/// Run every case against `target`, one after the other, each on a fresh instance.
///
/// A case that panics (a backend that panics, say) is reported as failed. A backend that never
/// answers hangs the run: the suite starts no timer, so a deadline is the caller's to set.
pub async fn run_conformance<T: ConformanceTarget>(target: &T) -> ConformanceReport {
    run(target, |_| true).await
}

/// Run only the cases whose ids are listed. Ids that match no case are ignored.
pub async fn run_cases<T: ConformanceTarget>(target: &T, ids: &[&str]) -> ConformanceReport {
    run(target, |case| ids.contains(&case.id)).await
}

/// Every case of the suite, with its stable id, group, class and description.
pub fn catalogue() -> Vec<Case> {
    cases::all().into_iter().map(|def| def.case).collect()
}

async fn run<T: ConformanceTarget>(target: &T, keep: impl Fn(&Case) -> bool) -> ConformanceReport {
    let source: &dyn Source = target;
    let lab = Lab { source };
    let mut results = Vec::new();
    for CaseDef { case, run } in cases::all() {
        if !keep(&case) {
            continue;
        }
        let verdict = Guarded((run)(&lab)).await;
        results.push(CaseResult {
            case,
            outcome: match verdict {
                Ok(()) => CaseOutcome::Passed,
                Err(Stop::Fail(reason)) => CaseOutcome::Failed(tidy(&reason)),
                Err(Stop::Skip(reason)) => CaseOutcome::NotApplicable(tidy(&reason)),
            },
        });
    }
    ConformanceReport {
        target: tidy(&target.name()),
        cases: results,
    }
}

/// Keep a reason short and on one line.
fn tidy(reason: &str) -> String {
    reason
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_REASON_CHARS)
        .collect()
}

/// A case whose panic is a failure, not the end of the run.
struct Guarded<F>(F);

impl<F: Future<Output = Verdict> + Unpin> Future for Guarded<F> {
    type Output = Verdict;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Verdict> {
        match catch_unwind(AssertUnwindSafe(|| Pin::new(&mut self.0).poll(cx))) {
            Ok(poll) => poll,
            Err(_) => Poll::Ready(Err(Stop::Fail("the case panicked".to_owned()))),
        }
    }
}
