//! The observability seams: [`Tracer`], [`MetricCollector`], and
//! [`ObservabilityBus`] — the Rust port of `@ailu/observability`'s
//! `interfaces.ts`.
//!
//! The TS interfaces are synchronous; the Rust traits mirror that (no `async`).
//! All methods take `&self` so in-memory implementations can use interior
//! mutability and remain `Send + Sync`.

use std::collections::BTreeMap;

use crate::types::{Metric, ObservabilityEvent, Span, SpanId, SpanStatus, TraceId};

/// A metric to record, without its `timestamp` — the Rust analogue of the TS
/// `Omit<Metric, "timestamp">` argument to [`MetricCollector::record`].
#[derive(Clone, Debug, PartialEq)]
pub struct MetricInput {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub tags: BTreeMap<String, String>,
}

/// Cancels a subscription registered via [`ObservabilityBus::subscribe`].
/// Calling it removes the handler — the Rust analogue of the TS `() => void`.
pub type Unsubscribe = Box<dyn Fn() + Send + Sync>;

/// A handler invoked for each event emitted on an [`ObservabilityBus`].
pub type EventHandler = Box<dyn Fn(&ObservabilityEvent) + Send + Sync>;

/// Starts, ends, and looks up spans within traces.
pub trait Tracer: Send + Sync {
    /// Start a new span. When `parent_span_id` resolves to a known span, the new
    /// span joins that span's trace; otherwise a fresh trace is created.
    fn start_span(
        &self,
        name: &str,
        attrs: BTreeMap<String, serde_json::Value>,
        parent_span_id: Option<SpanId>,
    ) -> Span;

    /// Mark a span as ended with the given status (and optional error). A no-op
    /// when the span is unknown — mirroring the TS early return.
    fn end_span(&self, span_id: &SpanId, status: SpanStatus, error: Option<String>);

    /// Look up a span by id.
    fn get_span(&self, span_id: &SpanId) -> Option<Span>;

    /// All spans belonging to a trace.
    fn get_trace(&self, trace_id: &TraceId) -> Vec<Span>;
}

/// Records metrics and queries them back by name and tags.
pub trait MetricCollector: Send + Sync {
    /// Record a metric, stamping it with the current time.
    fn record(&self, metric: MetricInput);

    /// Return all recorded metrics with the given `name`. When `tags` is
    /// provided, only metrics whose tags are a superset of every queried
    /// `(key, value)` pair are returned.
    fn query(&self, name: &str, tags: Option<&BTreeMap<String, String>>) -> Vec<Metric>;
}

/// Fans events out to every subscribed handler.
pub trait ObservabilityBus: Send + Sync {
    /// Deliver an event to all current subscribers.
    fn emit(&self, event: &ObservabilityEvent);

    /// Register a handler; the returned [`Unsubscribe`] removes it.
    fn subscribe(&self, handler: EventHandler) -> Unsubscribe;
}
