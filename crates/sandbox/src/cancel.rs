//! Cooperative cancellation handed to a backend.

use std::sync::Arc;

/// Cooperative cancellation check, in the style of the runtime's `CancelCheck`: a shared
/// closure that answers "should this work stop?".
///
/// The guard consults it before every call to the backend; a backend consults it during
/// `create`, `exec` and `collect` and answers `Cancelled`. `destroy` is not cancellable.
#[derive(Clone)]
pub struct CancelToken(Arc<dyn Fn() -> bool + Send + Sync>);

impl CancelToken {
    /// Wrap a check that returns `true` once cancellation was requested.
    pub fn new(check: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self(check)
    }

    /// A token that is never cancelled.
    pub fn never() -> Self {
        Self(Arc::new(|| false))
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        (self.0)()
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::never()
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn never_is_not_cancelled() {
        assert!(!CancelToken::never().is_cancelled());
        assert!(!CancelToken::default().is_cancelled());
    }

    #[test]
    fn follows_the_wrapped_check() {
        let flag = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&flag);
        let token = CancelToken::new(Arc::new(move || seen.load(Ordering::SeqCst)));
        let copy = token.clone();
        assert!(!token.is_cancelled());
        flag.store(true, Ordering::SeqCst);
        assert!(token.is_cancelled() && copy.is_cancelled());
    }
}
