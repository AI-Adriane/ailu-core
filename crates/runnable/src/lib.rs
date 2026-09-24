//! Rust port of `@ailu-ai/runnable` — the composable `Runnable` abstraction.
//!
//! A [`Runnable<I, O>`] is a unit of work that asynchronously transforms an
//! input into an output. Runnables compose:
//!
//! - [`RunnableLambda`] wraps an (async or sync) closure;
//! - [`RunnablePassthrough`] returns its input unchanged;
//! - [`RunnableSequence`] (built via [`then`]) chains `A: Runnable<I, M>` then
//!   `B: Runnable<M, O>` into a `Runnable<I, O>`, threading the value
//!   left-to-right;
//! - [`RunnableParallel`] runs N named branches against the **same** input
//!   concurrently and merges their outputs into a `BTreeMap<String, Value>`.
//!
//! All fallibility is explicit via [`RunnableError`]: a thrown TS step error
//! maps to [`Err`], and the first error in a sequence or parallel call
//! short-circuits — faithful to the TS `await` / `Promise.all` behaviour.
//!
//! ## Ergonomic divergences from the TS package (behaviour is identical)
//!
//! - The TS `Runnable` interface bundles `stream`/`batch`/`pipe`/`withRetry`/
//!   `withFallbacks` as methods. The Rust trait keeps only `invoke` required and
//!   `batch` as a default, and offers composition through explicit constructors
//!   (`then` / `RunnableSequence::new`, `RunnableParallel::with_branch`) rather
//!   than fluent methods, because Rust generics cannot express a `pipe<TNext>`
//!   returning a new boxed runnable without erasing the engine's types. See the
//!   [`runnable`] and [`sequence`] module docs for the full rationale.

#![forbid(unsafe_code)]

mod error;
mod lambda;
mod parallel;
mod passthrough;
mod runnable;
mod sequence;

pub use error::RunnableError;
pub use lambda::{sync_lambda, RunnableLambda};
pub use parallel::RunnableParallel;
pub use passthrough::RunnablePassthrough;
pub use runnable::Runnable;
pub use sequence::{then, RunnableSequence};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A lambda invokes its wrapped closure and returns the output.
    #[tokio::test]
    async fn lambda_invoke_returns_output() {
        let add_one = RunnableLambda::new(|input: Value| async move {
            let n = input
                .as_i64()
                .ok_or_else(|| RunnableError::step("not a number"))?;
            Ok(json!(n + 1))
        });

        let out = add_one.invoke(json!(41)).await.unwrap();
        assert_eq!(out, json!(42));
    }

    /// A passthrough returns its input unchanged.
    #[tokio::test]
    async fn passthrough_returns_input() {
        let pass = RunnablePassthrough::new();
        let input = json!({ "kept": true, "n": 7 });
        let out = pass.invoke(input.clone()).await.unwrap();
        assert_eq!(out, input);
    }

    /// A sequence pipes A -> B, threading A's output into B (assert the
    /// intermediate transform is applied before the second step).
    #[tokio::test]
    async fn sequence_pipes_a_into_b() {
        // A: n -> n + 1   B: n -> n * 2   so 2 -> 3 -> 6 (mirrors the TS test).
        let add_one = RunnableLambda::new(|input: Value| async move {
            let n = input
                .as_i64()
                .ok_or_else(|| RunnableError::step("not a number"))?;
            Ok(json!(n + 1))
        });
        let times_two = RunnableLambda::new(|input: Value| async move {
            let n = input
                .as_i64()
                .ok_or_else(|| RunnableError::step("not a number"))?;
            // If A's +1 had not run, this would be 2*2 = 4, not 6.
            Ok(json!(n * 2))
        });

        let pipeline = then(add_one, times_two);
        let out = pipeline.invoke(json!(2)).await.unwrap();
        assert_eq!(out, json!(6));
    }

    /// Sequences nest to form pipelines longer than two steps, left-to-right.
    #[tokio::test]
    async fn sequence_nests_for_longer_pipelines() {
        let add_one =
            RunnableLambda::new(
                |input: Value| async move { Ok(json!(input.as_i64().unwrap() + 1)) },
            );
        let times_two =
            RunnableLambda::new(
                |input: Value| async move { Ok(json!(input.as_i64().unwrap() * 2)) },
            );
        let minus_three =
            RunnableLambda::new(
                |input: Value| async move { Ok(json!(input.as_i64().unwrap() - 3)) },
            );

        // ((2 + 1) * 2) - 3 = 3.
        let pipeline = then(then(add_one, times_two), minus_three);
        let out = pipeline.invoke(json!(2)).await.unwrap();
        assert_eq!(out, json!(3));
    }

    /// Parallel runs every branch against the same input and merges results;
    /// assert all branches ran and each saw the identical input.
    #[tokio::test]
    async fn parallel_runs_all_branches_on_same_input() {
        let ran = Arc::new(AtomicUsize::new(0));

        let mk = |delta: i64, ran: Arc<AtomicUsize>| {
            RunnableLambda::new(move |input: Value| {
                let ran = Arc::clone(&ran);
                async move {
                    ran.fetch_add(1, Ordering::SeqCst);
                    let n = input
                        .as_i64()
                        .ok_or_else(|| RunnableError::step("not a number"))?;
                    Ok(json!(n + delta))
                }
            })
        };

        let parallel = RunnableParallel::new()
            .with_branch("add", mk(1, Arc::clone(&ran)))
            .with_branch(
                "mul",
                RunnableLambda::new(|input: Value| async move {
                    Ok(json!(input.as_i64().unwrap() * 3))
                }),
            );

        // Same input (4) feeds both branches: add -> 5, mul -> 12 (mirrors TS).
        let out = parallel.invoke(json!(4)).await.unwrap();

        let mut expected = BTreeMap::new();
        expected.insert("add".to_string(), json!(5));
        expected.insert("mul".to_string(), json!(12));
        assert_eq!(out, expected);
        // The `add` branch's counter confirms it executed.
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(parallel.len(), 2);
    }

    /// An error in a step propagates as Err through a sequence.
    #[tokio::test]
    async fn sequence_propagates_step_error() {
        let boom = RunnableLambda::new(|_input: Value| async move {
            Err::<Value, _>(RunnableError::step("boom"))
        });
        let never = RunnableLambda::new(|input: Value| async move { Ok(input) });

        let pipeline = then(boom, never);
        let err = pipeline.invoke(json!(1)).await.unwrap_err();
        match err {
            RunnableError::Step(msg) => assert_eq!(msg, "boom"),
            other => panic!("expected Step error, got {other:?}"),
        }
    }

    /// An error in any parallel branch propagates as Err from the whole call.
    #[tokio::test]
    async fn parallel_propagates_branch_error() {
        let parallel = RunnableParallel::new()
            .with_branch(
                "ok",
                RunnableLambda::new(|input: Value| async move { Ok(input) }),
            )
            .with_branch(
                "bad",
                RunnableLambda::new(|_input: Value| async move {
                    Err::<Value, _>(RunnableError::step("branch failed"))
                }),
            );

        let err = parallel.invoke(json!(1)).await.unwrap_err();
        match err {
            RunnableError::Step(msg) => assert_eq!(msg, "branch failed"),
            other => panic!("expected Step error, got {other:?}"),
        }
    }

    /// A synchronous lambda built via `sync_lambda` resolves immediately.
    #[tokio::test]
    async fn sync_lambda_resolves_eagerly() {
        let upper = sync_lambda(|input: Value| {
            let s = input
                .as_str()
                .ok_or_else(|| RunnableError::step("not a string"))?;
            Ok(json!(s.to_uppercase()))
        });
        let out = upper.invoke(json!("hello")).await.unwrap();
        assert_eq!(out, json!("HELLO"));
    }

    /// `batch` invokes the runnable once per input, preserving order.
    #[tokio::test]
    async fn batch_runs_each_input() {
        let add_one =
            RunnableLambda::new(
                |input: Value| async move { Ok(json!(input.as_i64().unwrap() + 1)) },
            );
        let out = add_one
            .batch(vec![json!(1), json!(2), json!(3)])
            .await
            .unwrap();
        assert_eq!(out, vec![json!(2), json!(3), json!(4)]);
    }
}
