//! In-memory [`Tracer`] — the Rust port of the TS `InMemoryTracer`.
//!
//! Spans are kept behind a [`Mutex`] so the tracer is `Send + Sync` and its
//! methods take `&self`. Insertion order is preserved (like the TS `Map`) so
//! [`Tracer::get_trace`] returns spans in the order they were started.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::Value;

use crate::interfaces::Tracer;
use crate::time::now_iso8601;
use crate::types::{Span, SpanId, SpanStatus, TraceId};

/// An in-memory tracer that stores every span it sees.
///
/// Faithful to `@ailu-ai/observability`'s `InMemoryTracer`: `start_span`
/// inherits the parent's `traceId` when the parent is known (otherwise mints a
/// new trace), and `end_span` is a no-op for unknown spans.
#[derive(Default)]
pub struct InMemoryTracer {
    /// `(spanId, span)` pairs in insertion order, like a JS `Map`.
    spans: Mutex<Vec<(SpanId, Span)>>,
    /// Monotonic counter making each generated id unique even within the same
    /// millisecond — the dependency-free analogue of the TS `Math.random()`
    /// suffix.
    seq: AtomicU64,
}

impl InMemoryTracer {
    /// Create an empty tracer.
    pub fn new() -> Self {
        Self::default()
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    fn new_span_id(&self) -> SpanId {
        SpanId(format!("span-{}-{}", now_millis(), self.next_seq()))
    }

    fn new_trace_id(&self) -> TraceId {
        TraceId(format!("trace-{}-{}", now_millis(), self.next_seq()))
    }
}

/// Current time as epoch milliseconds, mirroring the TS `Date.now()` used to
/// build span/trace ids.
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

impl Tracer for InMemoryTracer {
    fn start_span(
        &self,
        name: &str,
        attrs: BTreeMap<String, Value>,
        parent_span_id: Option<SpanId>,
    ) -> Span {
        let mut spans = self.spans.lock().expect("tracer mutex poisoned");

        let trace_id = parent_span_id
            .as_ref()
            .and_then(|pid| spans.iter().find(|(id, _)| id == pid))
            .map(|(_, parent)| parent.trace_id.clone())
            .unwrap_or_else(|| self.new_trace_id());

        let span = Span {
            id: self.new_span_id(),
            trace_id,
            parent_span_id,
            name: name.to_owned(),
            run_id: None,
            node_id: None,
            started_at: now_iso8601(),
            ended_at: None,
            status: SpanStatus::Ok,
            attributes: attrs,
            error: None,
        };

        spans.push((span.id.clone(), span.clone()));
        span
    }

    fn end_span(&self, span_id: &SpanId, status: SpanStatus, error: Option<String>) {
        let mut spans = self.spans.lock().expect("tracer mutex poisoned");
        if let Some((_, span)) = spans.iter_mut().find(|(id, _)| id == span_id) {
            span.ended_at = Some(now_iso8601());
            span.status = status;
            span.error = error;
        }
    }

    fn get_span(&self, span_id: &SpanId) -> Option<Span> {
        let spans = self.spans.lock().expect("tracer mutex poisoned");
        spans
            .iter()
            .find(|(id, _)| id == span_id)
            .map(|(_, span)| span.clone())
    }

    fn get_trace(&self, trace_id: &TraceId) -> Vec<Span> {
        let spans = self.spans.lock().expect("tracer mutex poisoned");
        spans
            .iter()
            .filter(|(_, span)| &span.trace_id == trace_id)
            .map(|(_, span)| span.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_end_span_updates_lifecycle_fields() {
        let tracer = InMemoryTracer::new();
        let mut attrs = BTreeMap::new();
        attrs.insert("phase".to_owned(), Value::from("start"));
        let span = tracer.start_span("graph.run", attrs, None);

        tracer.end_span(&span.id, SpanStatus::Ok, None);

        let ended = tracer.get_span(&span.id);
        assert!(ended.is_some());
        let ended = ended.unwrap();
        assert_eq!(ended.status, SpanStatus::Ok);
        assert!(ended.ended_at.is_some());
    }

    #[test]
    fn supports_parent_child_spans() {
        let tracer = InMemoryTracer::new();
        let parent = tracer.start_span("parent", BTreeMap::new(), None);
        let mut attrs = BTreeMap::new();
        attrs.insert("nested".to_owned(), Value::Bool(true));
        let child = tracer.start_span("child", attrs, Some(parent.id.clone()));

        assert_eq!(child.parent_span_id, Some(parent.id.clone()));
        assert_eq!(child.trace_id, parent.trace_id);
    }

    #[test]
    fn get_trace_returns_all_spans_in_same_trace() {
        let tracer = InMemoryTracer::new();
        let root = tracer.start_span("root", BTreeMap::new(), None);
        let child1 = tracer.start_span("child1", BTreeMap::new(), Some(root.id.clone()));
        let child2 = tracer.start_span("child2", BTreeMap::new(), Some(root.id.clone()));
        tracer.start_span("other-trace", BTreeMap::new(), None);

        let trace_spans = tracer.get_trace(&root.trace_id);
        let ids: Vec<&SpanId> = trace_spans.iter().map(|span| &span.id).collect();

        assert!(ids.contains(&&root.id));
        assert!(ids.contains(&&child1.id));
        assert!(ids.contains(&&child2.id));
        assert_eq!(trace_spans.len(), 3);
    }

    #[test]
    fn end_span_is_a_noop_for_unknown_spans() {
        let tracer = InMemoryTracer::new();
        // Must not panic and must record nothing.
        tracer.end_span(
            &SpanId::from("missing"),
            SpanStatus::Error,
            Some("x".into()),
        );
        assert!(tracer.get_span(&SpanId::from("missing")).is_none());
    }
}
