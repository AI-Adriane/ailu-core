//! Injected time source of the sandbox port.

/// Millisecond clock injected into the guard (and into any backend that needs time).
///
/// It is deliberately distinct from the run-event clock of the runtime: the sandbox port reads
/// time only through this trait, so the engine itself never reads the system clock here and a
/// test can drive time by hand.
pub trait SandboxClock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
}
