//! Executor-free helpers to overlap calls: the crate has no runtime, and the suite must run
//! under whichever one the caller has.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// A boxed future the helpers can hold across polls.
pub(crate) type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Poll `future` once with a waker that wakes nobody. `Some` when it finished; `None` when it
/// is waiting. Only for a call that needs nothing from the outside to finish, such as a
/// refusal by the guard: it must answer on the first poll.
pub(crate) fn poll_once<T>(future: &mut BoxFut<'_, T>) -> Option<T> {
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}

/// Drive `future` with the context of the case itself, so that it is woken like any other
/// call, until it finishes (`Some`) or until `reached` says it is held where the case wants
/// it (`None`). A call that yields before it gets there is simply driven on. The future
/// keeps its place and can be awaited afterwards.
pub(crate) async fn until<T>(future: &mut BoxFut<'_, T>, reached: impl Fn() -> bool) -> Option<T> {
    std::future::poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Ready(value) => Poll::Ready(Some(value)),
        Poll::Pending if reached() => Poll::Ready(None),
        Poll::Pending => Poll::Pending,
    })
    .await
}
