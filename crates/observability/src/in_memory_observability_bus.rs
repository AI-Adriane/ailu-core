//! In-memory [`ObservabilityBus`] — the Rust port of the TS
//! `InMemoryObservabilityBus`.
//!
//! Handlers live in an [`Arc`]-shared [`Mutex`] so the returned [`Unsubscribe`]
//! closure can remove its handler later without borrowing the bus. Each handler
//! gets a unique id (the TS relies on `Set` identity, which Rust closures lack).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::interfaces::{EventHandler, ObservabilityBus, Unsubscribe};
use crate::types::ObservabilityEvent;

type Registry = Mutex<Vec<(u64, EventHandler)>>;

/// An in-memory bus that fans every emitted event out to all current
/// subscribers, in subscription order.
///
/// Faithful to `@ailu-ai/observability`'s `InMemoryObservabilityBus`:
/// `subscribe` returns an unsubscribe handle that detaches exactly that
/// handler, and double-unsubscribe is harmless.
#[derive(Clone, Default)]
pub struct InMemoryObservabilityBus {
    handlers: Arc<Registry>,
    next_id: Arc<AtomicU64>,
}

impl InMemoryObservabilityBus {
    /// Create an empty bus.
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObservabilityBus for InMemoryObservabilityBus {
    fn emit(&self, event: &ObservabilityEvent) {
        let handlers = self.handlers.lock().expect("bus mutex poisoned");
        for (_, handler) in handlers.iter() {
            handler(event);
        }
    }

    fn subscribe(&self, handler: EventHandler) -> Unsubscribe {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.handlers
            .lock()
            .expect("bus mutex poisoned")
            .push((id, handler));

        let handlers = Arc::clone(&self.handlers);
        Box::new(move || {
            handlers
                .lock()
                .expect("bus mutex poisoned")
                .retain(|(hid, _)| *hid != id);
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::types::{Metric, SpanStatus};
    use crate::{Span, SpanId, TraceId};
    use std::collections::BTreeMap;

    fn sample_metric() -> ObservabilityEvent {
        ObservabilityEvent::Metric(Metric {
            name: "runtime.duration".to_owned(),
            value: 1.0,
            unit: "ms".to_owned(),
            tags: BTreeMap::new(),
            timestamp: "2026-06-11T00:00:00.000Z".to_owned(),
        })
    }

    fn sample_span() -> ObservabilityEvent {
        ObservabilityEvent::Span(Span {
            id: SpanId::from("span-1"),
            trace_id: TraceId::from("trace-1"),
            parent_span_id: None,
            name: "graph.run".to_owned(),
            run_id: None,
            node_id: None,
            started_at: "2026-06-11T00:00:00.000Z".to_owned(),
            ended_at: None,
            status: SpanStatus::Ok,
            attributes: BTreeMap::new(),
            error: None,
        })
    }

    #[test]
    fn fans_events_out_to_all_subscribers() {
        let bus = InMemoryObservabilityBus::new();
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));

        let a2 = Arc::clone(&a);
        let _unsub_a = bus.subscribe(Box::new(move |_event| {
            a2.fetch_add(1, Ordering::SeqCst);
        }));
        let b2 = Arc::clone(&b);
        let _unsub_b = bus.subscribe(Box::new(move |_event| {
            b2.fetch_add(1, Ordering::SeqCst);
        }));

        bus.emit(&sample_metric());
        bus.emit(&sample_span());

        assert_eq!(a.load(Ordering::SeqCst), 2);
        assert_eq!(b.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn unsubscribe_stops_delivery_to_that_handler_only() {
        let bus = InMemoryObservabilityBus::new();
        let kept = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));

        let kept2 = Arc::clone(&kept);
        let _unsub_kept = bus.subscribe(Box::new(move |_event| {
            kept2.fetch_add(1, Ordering::SeqCst);
        }));
        let dropped2 = Arc::clone(&dropped);
        let unsub = bus.subscribe(Box::new(move |_event| {
            dropped2.fetch_add(1, Ordering::SeqCst);
        }));

        bus.emit(&sample_metric());
        unsub();
        // Double-unsubscribe must be harmless.
        unsub();
        bus.emit(&sample_metric());

        assert_eq!(kept.load(Ordering::SeqCst), 2);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}
