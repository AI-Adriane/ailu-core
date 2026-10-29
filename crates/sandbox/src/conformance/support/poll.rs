//! Executor-free helpers to overlap calls: the crate has no runtime, and the suite must run
//! under whichever one the caller has.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// A boxed future the helpers can hold across polls.
pub(crate) type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Poll `future` once with a waker that wakes nobody. `Some` when it finished; `None` when it
/// is waiting (it keeps its place, and can be polled again).
pub(crate) fn poll_once<T>(future: &mut BoxFut<'_, T>) -> Option<T> {
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}
