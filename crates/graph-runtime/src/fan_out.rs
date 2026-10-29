//! Host-owned safety limits for dynamic fan-out. Admission happens before cloning items or
//! constructing child futures; a partitioned traversal budget also bounds nested expansion.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Maximum input cardinality for either dynamic map kind. Oversized maps fail, never truncate.
pub const DEFAULT_FAN_OUT_ITEMS: usize = 128;
pub const MAX_FAN_OUT_ITEMS: usize = 1024;
/// Maximum in-flight futures in one map. Aggregate admission bounds nested maps as well,
/// without holding a semaphore permit while a parent awaits its descendants.
pub const DEFAULT_FAN_OUT_CONCURRENCY: usize = 8;
pub const MAX_FAN_OUT_CONCURRENCY: usize = 32;
/// Maximum admitted items across all maps during one start/resume/replay traversal.
/// Resuming a checkpoint starts a new bounded traversal, including outstanding children.
pub const DEFAULT_FAN_OUT_TOTAL: usize = 512;
pub const MAX_FAN_OUT_TOTAL: usize = 4096;
/// Bound recursive expansion before a deeply nested future can exhaust the stack.
pub const MAX_FAN_OUT_DEPTH: usize = 8;

/// A host configuration value outside the engine's supported safety range.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("AILU_INVALID_FAN_OUT_LIMITS: fan-out limit '{limit}' must be between 1 and {ceiling}, got {value}")]
pub struct FanOutLimitsError {
    pub limit: &'static str,
    pub value: usize,
    pub ceiling: usize,
}

/// Validated host/runtime options. Private fields prevent bypassing construction checks.
/// Never read these limits from graph definitions, metadata, channels or saved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "FanOutLimitsConfig")]
pub struct FanOutLimits {
    max_items: usize,
    max_concurrency: usize,
    max_total_items: usize,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct FanOutLimitsConfig {
    max_items: usize,
    max_concurrency: usize,
    max_total_items: usize,
}

impl Default for FanOutLimitsConfig {
    fn default() -> Self {
        Self {
            max_items: DEFAULT_FAN_OUT_ITEMS,
            max_concurrency: DEFAULT_FAN_OUT_CONCURRENCY,
            max_total_items: DEFAULT_FAN_OUT_TOTAL,
        }
    }
}

impl TryFrom<FanOutLimitsConfig> for FanOutLimits {
    type Error = FanOutLimitsError;
    fn try_from(value: FanOutLimitsConfig) -> Result<Self, Self::Error> {
        Self::new(
            value.max_items,
            value.max_concurrency,
            value.max_total_items,
        )
    }
}

impl Default for FanOutLimits {
    fn default() -> Self {
        Self {
            max_items: DEFAULT_FAN_OUT_ITEMS,
            max_concurrency: DEFAULT_FAN_OUT_CONCURRENCY,
            max_total_items: DEFAULT_FAN_OUT_TOTAL,
        }
    }
}

impl FanOutLimits {
    pub fn new(
        max_items: usize,
        max_concurrency: usize,
        max_total_items: usize,
    ) -> Result<Self, FanOutLimitsError> {
        for (limit, value, ceiling) in [
            ("maxItems", max_items, MAX_FAN_OUT_ITEMS),
            ("maxConcurrency", max_concurrency, MAX_FAN_OUT_CONCURRENCY),
            ("maxTotalItems", max_total_items, MAX_FAN_OUT_TOTAL),
        ] {
            if value == 0 || value > ceiling {
                return Err(FanOutLimitsError {
                    limit,
                    value,
                    ceiling,
                });
            }
        }
        Ok(Self {
            max_items,
            max_concurrency,
            max_total_items,
        })
    }
}

#[derive(Clone)]
struct Scope {
    remaining: Arc<AtomicUsize>,
    depth: usize,
    limits: FanOutLimits,
}

impl Scope {
    fn root(limits: FanOutLimits) -> Self {
        Self {
            remaining: Arc::new(AtomicUsize::new(limits.max_total_items)),
            depth: 0,
            limits,
        }
    }
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
    with_fan_out_limits_scope(future, FanOutLimits::default())
}

pub(crate) fn with_fan_out_limits_scope<F: Future>(
    future: F,
    limits: FanOutLimits,
) -> impl Future<Output = F::Output> {
    let captured = CURRENT.with(|slot| slot.borrow().clone());
    scoped(future, captured, limits)
}

fn scoped<F: Future>(
    future: F,
    mut scope: Option<Scope>,
    limits: FanOutLimits,
) -> impl Future<Output = F::Output> {
    let mut future = Box::pin(future);
    std::future::poll_fn(move |cx| {
        let budget = scope.get_or_insert_with(|| {
            CURRENT.with(|slot| slot.borrow().clone().unwrap_or_else(|| Scope::root(limits)))
        });
        let _restore = Restore(CURRENT.with(|slot| slot.replace(Some(budget.clone()))));
        future.as_mut().poll(cx)
    })
}

/// Scheduling limit from the active trusted scope (safe defaults for native direct callers).
#[doc(hidden)]
pub fn fan_out_concurrency() -> usize {
    CURRENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|scope| scope.limits)
            .unwrap_or_default()
            .max_concurrency
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
            .unwrap_or_else(|| Scope::root(FanOutLimits::default()));
        let remaining = parent.remaining.swap(0, Ordering::Relaxed);
        let children = (0..count)
            .map(|index| Scope {
                remaining: Arc::new(AtomicUsize::new(
                    remaining / count + usize::from(index < remaining % count),
                )),
                depth: parent.depth + usize::from(dynamic),
                limits: parent.limits,
            })
            .collect();
        Self {
            parent: Some(parent.remaining),
            children,
        }
    }

    pub fn run<F: Future>(&self, index: usize, future: F) -> impl Future<Output = F::Output> {
        let child = self.children[index].clone();
        let limits = child.limits;
        scoped(future, Some(child), limits)
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
    CURRENT.with(|slot| {
        let current = slot.borrow();
        let limits = current.as_ref().map(|scope| scope.limits).unwrap_or_default();
        if count > limits.max_items {
            return Err(format!("got {count} items; limit is {}", limits.max_items));
        }
        if let Some(budget) = current.as_ref() {
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
                    "traversal item budget exceeded: branch has {remaining} items remaining (traversal limit {})", limits.max_total_items
                ))?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_rejects_each_ceiling_and_zero_with_a_typed_error() {
        for (values, field, ceiling) in [
            ((1025, 8, 512), "maxItems", 1024),
            ((128, 33, 512), "maxConcurrency", 32),
            ((128, 8, 4097), "maxTotalItems", 4096),
            ((0, 8, 512), "maxItems", 1024),
            ((128, 0, 512), "maxConcurrency", 32),
            ((128, 8, 0), "maxTotalItems", 4096),
        ] {
            let error = FanOutLimits::new(values.0, values.1, values.2).unwrap_err();
            assert_eq!(error.limit, field);
            assert_eq!(error.ceiling, ceiling);
        }
        FanOutLimits::new(1024, 32, 4096).unwrap();
        assert!(serde_json::from_str::<FanOutLimits>(r#"{"maxItems":1025}"#).is_err());
        assert_eq!(
            serde_json::from_str::<FanOutLimits>("{}").unwrap(),
            FanOutLimits::default()
        );
    }

    #[tokio::test]
    async fn configured_admission_is_inherited_and_cannot_be_raised_by_a_child() {
        let limits = FanOutLimits::new(256, 2, 1024).unwrap();
        with_fan_out_limits_scope(
            async {
                assert_eq!(fan_out_concurrency(), 2);
                for _ in 0..4 {
                    admit_items(256).unwrap();
                }
                assert!(admit_items(1).is_err());
                with_fan_out_limits_scope(
                    async {
                        assert_eq!(fan_out_concurrency(), 2);
                        assert!(admit_items(1).is_err());
                    },
                    FanOutLimits::new(1024, 32, 4096).unwrap(),
                )
                .await;
            },
            limits,
        )
        .await;
    }

    fn exhaust_admission() {
        for _ in 0..DEFAULT_FAN_OUT_TOTAL / DEFAULT_FAN_OUT_ITEMS {
            admit_items(DEFAULT_FAN_OUT_ITEMS).unwrap();
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
                        admit_items(if index == 0 { 64 } else { 128 }).is_ok()
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
            for _ in 0..DEFAULT_FAN_OUT_TOTAL / DEFAULT_FAN_OUT_ITEMS {
                admit_items(DEFAULT_FAN_OUT_ITEMS).unwrap();
                let batch = FanOutBatch::new(DEFAULT_FAN_OUT_ITEMS, true);
                futures_util::future::join_all(
                    (0..DEFAULT_FAN_OUT_ITEMS).map(|index| batch.run(index, async {})),
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
            assert!(admit_items(DEFAULT_FAN_OUT_ITEMS + 1).is_err());
            for _ in 0..DEFAULT_FAN_OUT_TOTAL / DEFAULT_FAN_OUT_ITEMS {
                with_fan_out_scope(async {
                    tokio::task::yield_now().await;
                    admit_items(DEFAULT_FAN_OUT_ITEMS).unwrap();
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
                for _ in 0..DEFAULT_FAN_OUT_TOTAL / DEFAULT_FAN_OUT_ITEMS {
                    admit_items(DEFAULT_FAN_OUT_ITEMS).unwrap();
                    tokio::task::yield_now().await;
                }
                assert!(admit_items(1).is_err());
            })
        };
        futures_util::future::join_all([root(), root()]).await;
        with_fan_out_scope(async { admit_items(DEFAULT_FAN_OUT_ITEMS).unwrap() }).await;
    }
}
