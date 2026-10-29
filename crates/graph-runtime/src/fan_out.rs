//! Fixed safety limits for dynamic fan-out. Admission happens before cloning items or
//! constructing child futures; a partitioned traversal budget also bounds nested expansion.

use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Maximum input cardinality for either dynamic map kind. Oversized maps fail, never truncate.
pub const MAX_FAN_OUT_ITEMS: usize = 64;
/// Maximum in-flight futures in one map. Aggregate admission bounds nested maps as well,
/// without holding a semaphore permit while a parent awaits its descendants.
pub const MAX_FAN_OUT_CONCURRENCY: usize = 8;
/// Maximum admitted items across all maps during one start/resume/replay traversal.
/// Resuming a checkpoint starts a new bounded traversal, including outstanding children.
pub const MAX_FAN_OUT_TOTAL: usize = 256;
/// Bound recursive expansion before a deeply nested future can exhaust the stack.
pub const MAX_FAN_OUT_DEPTH: usize = 8;

#[derive(Clone)]
struct Scope {
    remaining: Arc<AtomicUsize>,
    depth: usize,
}

std::thread_local! {
    static CURRENT: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

struct Restore(Option<Scope>);

impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|slot| slot.replace(self.0.take()));
    }
}

/// Share admission across the runtime and native map handlers without exposing trusted
/// accounting in GraphState channels. Direct native handlers get the same safe defaults.
///
/// This is an executor-independent future scope: the thread-local is installed ONLY while
/// polling, restored even on panic, and never held across an await. Independent futures on
/// one thread stay isolated, and a future moving threads carries its own Arc. Nested scopes
/// inherit the currently polled parent's budget, so subgraphs cannot reset admission.
///
/// Executors do not propagate this context into spawned tasks. Wrap the child's future
/// with this function BEFORE calling `spawn` (or moving it into `spawn_blocking`): the
/// wrapper captures the active scope at construction and carries it across that boundary.
/// Constructing the wrapper inside an unscoped spawned task instead creates a new root.
/// Native handlers that spawn work are trusted to preserve this boundary explicitly.
#[doc(hidden)]
pub fn with_fan_out_scope<F: Future>(future: F) -> impl Future<Output = F::Output> {
    let captured = CURRENT.with(|slot| slot.borrow().clone());
    scoped(future, captured)
}

fn scoped<F: Future>(future: F, mut scope: Option<Scope>) -> impl Future<Output = F::Output> {
    let mut future = Box::pin(future);
    std::future::poll_fn(move |cx| {
        let budget = scope.get_or_insert_with(|| {
            CURRENT.with(|slot| {
                slot.borrow().clone().unwrap_or_else(|| Scope {
                    remaining: Arc::new(AtomicUsize::new(MAX_FAN_OUT_TOTAL)),
                    depth: 0,
                })
            })
        });
        let _restore = Restore(CURRENT.with(|slot| slot.replace(Some(budget.clone()))));
        future.as_mut().poll(cx)
    })
}

/// Reserve the remaining allowance for concurrently executing branches, in input order.
/// A branch cannot spend a sibling's allowance. This is deliberately conservative: a map
/// may fail below the total cap when its own share is too small, but successful IO timing
/// cannot decide which items run or which output slots fail. Unused allowance returns to
/// the parent after the batch, so subsequent sequential maps can still use it.
#[doc(hidden)]
pub struct FanOutBatch {
    parent: Option<Arc<AtomicUsize>>,
    children: Vec<Scope>,
}

impl FanOutBatch {
    pub fn new(count: usize, dynamic: bool) -> Self {
        if count == 0 {
            return Self {
                parent: None,
                children: Vec::new(),
            };
        }
        let parent = CURRENT
            .with(|slot| slot.borrow().clone())
            .unwrap_or_else(|| Scope {
                remaining: Arc::new(AtomicUsize::new(MAX_FAN_OUT_TOTAL)),
                depth: 0,
            });
        let remaining = parent.remaining.swap(0, Ordering::Relaxed);
        let children = (0..count)
            .map(|index| Scope {
                remaining: Arc::new(AtomicUsize::new(
                    remaining / count + usize::from(index < remaining % count),
                )),
                depth: parent.depth + usize::from(dynamic),
            })
            .collect();
        Self {
            parent: Some(parent.remaining),
            children,
        }
    }

    pub fn run<F: Future>(&self, index: usize, future: F) -> impl Future<Output = F::Output> {
        scoped(future, Some(self.children[index].clone()))
    }
}

impl Drop for FanOutBatch {
    fn drop(&mut self) {
        if let Some(parent) = &self.parent {
            let unused: usize = self
                .children
                .iter()
                .map(|child| child.remaining.load(Ordering::Relaxed))
                .sum();
            parent.fetch_add(unused, Ordering::Relaxed);
        }
    }
}

pub(crate) fn admit_items(count: usize) -> Result<(), String> {
    if count > MAX_FAN_OUT_ITEMS {
        return Err(format!("got {count} items; limit is {MAX_FAN_OUT_ITEMS}"));
    }
    CURRENT.with(|slot| {
        if let Some(budget) = slot.borrow().as_ref() {
            if count > 0 && budget.depth >= MAX_FAN_OUT_DEPTH {
                return Err(format!(
                    "fan-out nesting limit of {MAX_FAN_OUT_DEPTH} exceeded"
                ));
            }
            budget
                .remaining
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                    remaining.checked_sub(count)
                })
                .map_err(|remaining| format!(
                    "traversal item budget exceeded: branch has {remaining} items remaining (traversal limit {MAX_FAN_OUT_TOTAL})"
                ))?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exhaust_admission() {
        for _ in 0..MAX_FAN_OUT_TOTAL / MAX_FAN_OUT_ITEMS {
            admit_items(MAX_FAN_OUT_ITEMS).unwrap();
        }
    }

    #[tokio::test]
    async fn a_scope_captured_before_spawn_preserves_admission() {
        with_fan_out_scope(async {
            exhaust_admission();
            let child = with_fan_out_scope(async { admit_items(1) });
            assert!(tokio::spawn(child).await.unwrap().is_err());
        })
        .await;
    }

    #[tokio::test]
    async fn a_scope_captured_before_spawn_blocking_preserves_admission() {
        with_fan_out_scope(async {
            exhaust_admission();
            let child = with_fan_out_scope(async { admit_items(1) });
            let runtime = tokio::runtime::Handle::current();
            assert!(tokio::task::spawn_blocking(move || runtime.block_on(child))
                .await
                .unwrap()
                .is_err());
        })
        .await;
    }

    #[tokio::test]
    async fn inheriting_a_scope_does_not_charge_dynamic_depth() {
        fn nested(depth: usize) -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>>>> {
            Box::pin(with_fan_out_scope(async move {
                if depth == 0 {
                    admit_items(1)
                } else {
                    nested(depth - 1).await
                }
            }))
        }
        nested(MAX_FAN_OUT_DEPTH + 1).await.unwrap();
    }

    #[tokio::test]
    async fn sibling_admission_does_not_depend_on_poll_order() {
        async fn run(delay: usize) -> Vec<bool> {
            with_fan_out_scope(async {
                admit_items(4).unwrap();
                let batch = FanOutBatch::new(4, true);
                futures_util::future::join_all((0..4).map(|index| {
                    batch.run(index, async move {
                        if index == delay {
                            tokio::task::yield_now().await;
                        }
                        admit_items(if index == 0 { 32 } else { 64 }).is_ok()
                    })
                }))
                .await
            })
            .await
        }
        assert_eq!(run(0).await, vec![true, false, false, false]);
        assert_eq!(run(0).await, run(3).await);
    }

    #[tokio::test]
    async fn unused_branch_allowance_returns_without_refunding_admitted_work() {
        with_fan_out_scope(async {
            for _ in 0..MAX_FAN_OUT_TOTAL / MAX_FAN_OUT_ITEMS {
                admit_items(MAX_FAN_OUT_ITEMS).unwrap();
                let batch = FanOutBatch::new(MAX_FAN_OUT_ITEMS, true);
                futures_util::future::join_all(
                    (0..MAX_FAN_OUT_ITEMS).map(|index| batch.run(index, async {})),
                )
                .await;
            }
            assert!(admit_items(1).is_err());
        })
        .await;
    }

    #[tokio::test]
    async fn nested_scopes_share_admission_and_rejection_does_not_consume_budget() {
        with_fan_out_scope(async {
            assert!(admit_items(MAX_FAN_OUT_ITEMS + 1).is_err());
            for _ in 0..MAX_FAN_OUT_TOTAL / MAX_FAN_OUT_ITEMS {
                with_fan_out_scope(async {
                    tokio::task::yield_now().await;
                    admit_items(MAX_FAN_OUT_ITEMS).unwrap();
                })
                .await;
            }
            assert!(with_fan_out_scope(async { admit_items(1) }).await.is_err());
            admit_items(0).unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn interleaved_roots_have_independent_budgets() {
        let root = || {
            with_fan_out_scope(async {
                for _ in 0..MAX_FAN_OUT_TOTAL / MAX_FAN_OUT_ITEMS {
                    admit_items(MAX_FAN_OUT_ITEMS).unwrap();
                    tokio::task::yield_now().await;
                }
                assert!(admit_items(1).is_err());
            })
        };
        futures_util::future::join_all([root(), root()]).await;
        with_fan_out_scope(async { admit_items(MAX_FAN_OUT_ITEMS).unwrap() }).await;
    }
}
