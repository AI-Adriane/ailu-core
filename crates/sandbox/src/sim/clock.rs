//! The virtual clock of the simulated backend.

use crate::clock::SandboxClock;
use std::sync::atomic::{AtomicU64, Ordering};

/// A clock that only moves when a test moves it.
///
/// Share one instance between the guard and the simulated backend (both take an
/// `Arc<dyn SandboxClock>`) and advance it by hand: a `ttl`, a retention window or a rate
/// window then expires exactly when the test says so, and no test ever sleeps.
#[derive(Debug, Default)]
pub struct SimClock {
    now_ms: AtomicU64,
}

impl SimClock {
    /// A clock that reads `start_ms` until it is advanced.
    pub fn new(start_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(start_ms),
        }
    }

    /// Move time forward by `ms` (saturating).
    pub fn advance(&self, ms: u64) {
        let _ = self
            .now_ms
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |now| {
                Some(now.saturating_add(ms))
            });
    }

    /// Set the time to `ms`, forward or backward.
    pub fn set(&self, ms: u64) {
        self.now_ms.store(ms, Ordering::SeqCst);
    }
}

impl SandboxClock for SimClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_moves_only_by_hand() {
        let clock = SimClock::new(1_000);
        assert_eq!(clock.now_ms(), 1_000);
        assert_eq!(clock.now_ms(), 1_000);
        clock.advance(250);
        assert_eq!(clock.now_ms(), 1_250);
        clock.set(10);
        assert_eq!(clock.now_ms(), 10);
        clock.set(u64::MAX - 1);
        clock.advance(100);
        assert_eq!(clock.now_ms(), u64::MAX);
    }
}
