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

/// The future of one case, as `run_conformance_with` hands it to the caller.
pub type CaseFuture<'a> = Pin<Box<dyn Future<Output = CaseOutcome> + Send + 'a>>;

/// Run every case against `target`, one after the other, each on a fresh instance.
///
/// A case that panics (a backend that panics, say) is reported as failed. A backend that never
/// answers hangs the run: the suite starts no timer, because it needs no runtime. Put a deadline
/// on each case with [`run_conformance_with`], using the timeout of your own executor.
pub async fn run_conformance<T: ConformanceTarget>(target: &T) -> ConformanceReport {
    run_conformance_with(target, |_, case| case).await
}

/// Like [`run_conformance`], but each case goes through `per_case` before it is awaited, so
/// that the caller can bound it. The outcome `per_case` returns is the one reported: for
/// instance, with Tokio, wrap the future in `tokio::time::timeout` and answer
/// `CaseOutcome::Failed("timed out".into())` when it elapses.
pub async fn run_conformance_with<T, F>(target: &T, per_case: F) -> ConformanceReport
where
    T: ConformanceTarget,
    F: for<'a> Fn(&Case, CaseFuture<'a>) -> CaseFuture<'a>,
{
    run(target, |_| true, per_case).await
}

/// Run only the cases whose ids are listed. Ids that match no case are ignored.
pub async fn run_cases<T: ConformanceTarget>(target: &T, ids: &[&str]) -> ConformanceReport {
    run(target, |case| ids.contains(&case.id), |_, case| case).await
}

/// Like [`run_cases`], with a way to bound each case: see [`run_conformance_with`].
pub async fn run_cases_with<T, F>(target: &T, ids: &[&str], per_case: F) -> ConformanceReport
where
    T: ConformanceTarget,
    F: for<'a> Fn(&Case, CaseFuture<'a>) -> CaseFuture<'a>,
{
    run(target, |case| ids.contains(&case.id), per_case).await
}

/// Every case of the suite, with its stable id, group, class and description.
pub fn catalogue() -> Vec<Case> {
    cases::all().into_iter().map(|def| def.case).collect()
}

async fn run<T, F>(target: &T, keep: impl Fn(&Case) -> bool, per_case: F) -> ConformanceReport
where
    T: ConformanceTarget,
    F: for<'a> Fn(&Case, CaseFuture<'a>) -> CaseFuture<'a>,
{
    let source: &dyn Source = target;
    let lab = Lab { source };
    let mut results = Vec::new();
    for CaseDef { case, run } in cases::all() {
        if !keep(&case) {
            continue;
        }
        let verdict = Guarded((run)(&lab));
        let outcome: CaseFuture<'_> = Box::pin(async move {
            match verdict.await {
                Ok(()) => CaseOutcome::Passed,
                Err(Stop::Fail(reason)) => CaseOutcome::Failed(tidy(&reason)),
                Err(Stop::Skip(reason)) => CaseOutcome::NotApplicable(tidy(&reason)),
                Err(Stop::Measured(reason)) => CaseOutcome::Measured(tidy(&reason)),
            }
        });
        let outcome = per_case(&case, outcome).await;
        results.push(CaseResult { case, outcome });
    }
    // A name that panics is no reason to lose the report.
    let name = catch_unwind(AssertUnwindSafe(|| target.name()))
        .unwrap_or_else(|_| "a target whose name panicked".to_owned());
    ConformanceReport {
        target: tidy(&name),
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
