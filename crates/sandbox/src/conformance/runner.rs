//! Running the cases against a target.

use super::cases::{self, CaseDef};
use super::report::{Case, CaseOutcome, CaseResult, ConformanceReport};
use super::support::{Lab, Source, Stop, Verdict};
use super::target::ConformanceTarget;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Waker;
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Most unknown ids a report keeps.
const MAX_UNKNOWN_IDS: usize = 20;

/// Longest reason kept in a report.
const MAX_REASON_CHARS: usize = 240;

/// The future of one case, as `run_conformance_with` hands it to the caller.
pub type CaseFuture<'a> = Pin<Box<dyn Future<Output = CaseOutcome> + Send + 'a>>;

/// How long a case may take before it is reported as failed, when the caller sets nothing:
/// sixty seconds of wall-clock time. A real backend starts sandboxes, so the bound is generous;
/// raise it, or lift it, with [`run_conformance_timeout`].
pub const DEFAULT_CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// Run every case against `target`, one after the other, each on a fresh instance and each
/// bounded by [`DEFAULT_CASE_TIMEOUT`]. A case that takes longer is reported as failed.
///
/// **What the bound can and cannot do.** A backend that never answers is reported as failed, but
/// only because the case is interrupted where it awaits. Code that blocks without awaiting (a
/// synchronous sleep, an infinite loop) cannot be interrupted from the same thread: a case that
/// ends after its limit is reported as failed whatever its result, never as passed, but a loop
/// that never ends still hangs the run. Stopping it would need every case on a thread of its
/// own that can be abandoned, which a case that borrows the target cannot be; put such a run
/// under a timeout of the process.
///
/// A case that panics (a backend that panics, say) is reported as failed. The bound uses one
/// helper thread per case and needs no runtime. To change it or to lift it, use
/// [`run_conformance_timeout`]; to bound the cases with your own executor, use
/// [`run_conformance_with`].
pub async fn run_conformance<T: ConformanceTarget>(target: &T) -> ConformanceReport {
    run_conformance_timeout(target, Some(DEFAULT_CASE_TIMEOUT)).await
}

/// Like [`run_conformance`], with the bound of each case chosen by the caller: `Some(limit)`
/// changes it, `None` lifts it (a backend that never answers then hangs the run).
pub async fn run_conformance_timeout<T: ConformanceTarget>(
    target: &T,
    limit: Option<Duration>,
) -> ConformanceReport {
    run(target, &[], |_| true, |_, case| timed(limit, case)).await
}

/// Like [`run_conformance`], but each case goes through `per_case` before it is awaited, so
/// that the caller can bound it with the timeout of its own executor. **The default bound does
/// not apply here**: `per_case` is the bound, and passing `|_, case| case` lifts it. The outcome
/// `per_case` returns is the one reported: for instance, with Tokio, wrap the future in
/// `tokio::time::timeout` and answer `CaseOutcome::Failed("timed out".into())` when it
/// elapses.
pub async fn run_conformance_with<T, F>(target: &T, per_case: F) -> ConformanceReport
where
    T: ConformanceTarget,
    F: for<'a> Fn(&Case, CaseFuture<'a>) -> CaseFuture<'a>,
{
    run(target, &[], |_| true, per_case).await
}

/// Run only the cases whose ids are listed, each bounded by [`DEFAULT_CASE_TIMEOUT`]. An id that
/// matches no case is not ignored: the report keeps it in `unknown_ids`, and a report that holds
/// one is never conformant. An id listed twice is not refused: the case runs once. The report
/// of such a run covers part of the catalogue, so it is never conformant either: it is for
/// looking at a few cases.
pub async fn run_cases<T: ConformanceTarget>(target: &T, ids: &[&str]) -> ConformanceReport {
    run_cases_timeout(target, ids, Some(DEFAULT_CASE_TIMEOUT)).await
}

/// Like [`run_cases`], with the bound of each case chosen by the caller: see
/// [`run_conformance_timeout`].
pub async fn run_cases_timeout<T: ConformanceTarget>(
    target: &T,
    ids: &[&str],
    limit: Option<Duration>,
) -> ConformanceReport {
    run(
        target,
        ids,
        |case| ids.contains(&case.id),
        |_, case| timed(limit, case),
    )
    .await
}

/// Like [`run_cases`], with a way to bound each case: see [`run_conformance_with`]. The default
/// bound does not apply here either.
pub async fn run_cases_with<T, F>(target: &T, ids: &[&str], per_case: F) -> ConformanceReport
where
    T: ConformanceTarget,
    F: for<'a> Fn(&Case, CaseFuture<'a>) -> CaseFuture<'a>,
{
    run(target, ids, |case| ids.contains(&case.id), per_case).await
}

/// Every case of the suite, with its stable id, group, class and description.
pub fn catalogue() -> Vec<Case> {
    cases::all().into_iter().map(|def| def.case).collect()
}

async fn run<T, F>(
    target: &T,
    asked: &[&str],
    keep: impl Fn(&Case) -> bool,
    per_case: F,
) -> ConformanceReport
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
    let known = catalogue();
    let unknown_ids = asked
        .iter()
        .filter(|id| !known.iter().any(|case| case.id == **id))
        .take(MAX_UNKNOWN_IDS)
        .map(|id| tidy(id))
        .collect();
    ConformanceReport {
        target: tidy(&name),
        cases: results,
        unknown_ids,
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

/// What the clock thread of a bounded case shares with the future.
struct Clock {
    expired: AtomicBool,
    done: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

/// A case bounded by a wall-clock limit. A helper thread sleeps until the limit and wakes the
/// task that polls the case, which then reports the failure; the thread is released as soon as
/// the case ends or is dropped. The suite needs no runtime and the bound needs none either.
struct Timed<'a> {
    inner: CaseFuture<'a>,
    limit: Duration,
    clock: Arc<Clock>,
    thread: Option<JoinHandle<()>>,
    started: bool,
    /// When the case was first polled: what the case really took, awaits or not.
    began: Option<Instant>,
}

/// Bound `case` by `limit` of wall-clock time; `None` leaves it as it is.
fn timed(limit: Option<Duration>, case: CaseFuture<'_>) -> CaseFuture<'_> {
    match limit {
        None => case,
        Some(limit) => Box::pin(Timed {
            inner: case,
            limit,
            clock: Arc::new(Clock {
                expired: AtomicBool::new(false),
                done: AtomicBool::new(false),
                waker: Mutex::new(None),
            }),
            thread: None,
            started: false,
            began: None,
        }),
    }
}

impl Timed<'_> {
    fn start(&mut self) -> bool {
        self.started = true;
        self.began = Some(Instant::now());
        let clock = Arc::clone(&self.clock);
        let deadline = Instant::now() + self.limit;
        let spawned = std::thread::Builder::new()
            .name("conformance-case-timer".to_owned())
            .spawn(move || loop {
                if clock.done.load(Ordering::SeqCst) {
                    return;
                }
                let now = Instant::now();
                if now >= deadline {
                    clock.expired.store(true, Ordering::SeqCst);
                    let waker = clock
                        .waker
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                    return;
                }
                std::thread::park_timeout(deadline - now);
            });
        match spawned {
            Ok(handle) => {
                self.thread = Some(handle);
                true
            }
            Err(_) => false,
        }
    }
}

impl Future for Timed<'_> {
    type Output = CaseOutcome;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<CaseOutcome> {
        if !self.started && !self.start() {
            return Poll::Ready(CaseOutcome::Failed(
                "the timer of the case could not be started".to_owned(),
            ));
        }
        // The waker is stored before the clock is read, so an expiry cannot be missed.
        *self
            .clock
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(cx.waker().clone());
        if self.clock.expired.load(Ordering::SeqCst) {
            return Poll::Ready(CaseOutcome::Failed(format!(
                "timed out after {} ms",
                self.limit.as_millis()
            )));
        }
        let polled = self.inner.as_mut().poll(cx);
        if let Poll::Ready(outcome) = polled {
            self.release();
            // Code that blocks without awaiting cannot be interrupted, but it can be told: a
            // case that ended after its limit is a failure whatever it concluded.
            let took = self.began.map_or(Duration::ZERO, |began| began.elapsed());
            if took > self.limit && !matches!(outcome, CaseOutcome::Failed(_)) {
                return Poll::Ready(CaseOutcome::Failed(format!(
                    "timed out: the case took {} ms, limit {} ms",
                    took.as_millis(),
                    self.limit.as_millis()
                )));
            }
            return Poll::Ready(outcome);
        }
        Poll::Pending
    }
}

impl Timed<'_> {
    fn release(&self) {
        self.clock.done.store(true, Ordering::SeqCst);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

impl Drop for Timed<'_> {
    fn drop(&mut self) {
        self.release();
    }
}
