//! Observability value types: [`Span`], [`Metric`], and the
//! [`ObservabilityEvent`] union — the Rust port of `@ailu-ai/observability`'s
//! `types.ts`.
//!
//! Timestamps mirror the TS `Date` fields as ISO-8601 / RFC-3339 UTC strings
//! (the shape `new Date().toISOString()` serialises to in JSON), matching the
//! sibling Rust ports (`graph-runtime`, `artifact-store`, `memory-store`).

use std::collections::BTreeMap;

use ailu_graph_core::{NodeId, RunId};
use ailu_graph_runtime::RunEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Identifier of a trace — the root grouping for a set of related spans.
///
/// Serialises transparently as a plain string, mirroring the TS
/// `Brand<string, "TraceId">`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TraceId(pub String);

impl TraceId {
    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TraceId {
    fn from(value: &str) -> Self {
        TraceId(value.to_owned())
    }
}

impl From<String> for TraceId {
    fn from(value: String) -> Self {
        TraceId(value)
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier of a single span within a trace.
///
/// Serialises transparently as a plain string, mirroring the TS
/// `Brand<string, "SpanId">`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpanId(pub String);

impl SpanId {
    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SpanId {
    fn from(value: &str) -> Self {
        SpanId(value.to_owned())
    }
}

impl From<String> for SpanId {
    fn from(value: String) -> Self {
        SpanId(value)
    }
}

impl std::fmt::Display for SpanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Terminal status of a span. Mirrors the TS `SPAN_STATUSES = ["ok", "error"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Ok,
    Error,
}

/// A single unit of work in a trace. Faithful to the TS `Span` type: optional
/// fields are omitted from the wire form when `None` (mirroring TS `undefined`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub id: SpanId,
    pub trace_id: TraceId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub node_id: Option<NodeId>,
    /// ISO-8601 / RFC-3339 UTC start time (TS `startedAt: Date`).
    pub started_at: String,
    /// ISO-8601 / RFC-3339 UTC end time, set on [`crate::Tracer::end_span`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ended_at: Option<String>,
    pub status: SpanStatus,
    /// Arbitrary span attributes (TS `Record<string, unknown>`). A `BTreeMap`
    /// keeps the serialised key order deterministic.
    pub attributes: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

/// A recorded measurement. Faithful to the TS `Metric` type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub unit: String,
    /// String-valued tags (TS `Record<string, string>`).
    pub tags: BTreeMap<String, String>,
    /// ISO-8601 / RFC-3339 UTC time the metric was recorded (TS `timestamp: Date`).
    pub timestamp: String,
}

/// Anything that can flow over the [`crate::ObservabilityBus`].
///
/// Mirrors the TS `ObservabilityEvent = RunEvent | Span | Metric`. Serialised
/// untagged so each variant keeps its own wire shape, exactly like the TS union.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ObservabilityEvent {
    Run(RunEvent),
    Span(Span),
    Metric(Metric),
}

impl From<RunEvent> for ObservabilityEvent {
    fn from(value: RunEvent) -> Self {
        ObservabilityEvent::Run(value)
    }
}

impl From<Span> for ObservabilityEvent {
    fn from(value: Span) -> Self {
        ObservabilityEvent::Span(value)
    }
}

impl From<Metric> for ObservabilityEvent {
    fn from(value: Metric) -> Self {
        ObservabilityEvent::Metric(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_id_serialises_transparently_as_a_string() {
        let id = SpanId::from("span-1");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"span-1\"");
        let back: SpanId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn span_status_serialises_as_lowercase() {
        assert_eq!(serde_json::to_string(&SpanStatus::Ok).unwrap(), "\"ok\"");
        assert_eq!(
            serde_json::to_string(&SpanStatus::Error).unwrap(),
            "\"error\""
        );
    }

    #[test]
    fn span_omits_none_fields_and_uses_camel_case() {
        let span = Span {
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
        };
        let json = serde_json::to_value(&span).unwrap();
        assert_eq!(json["traceId"], "trace-1");
        assert!(json.get("parentSpanId").is_none());
        assert!(json.get("endedAt").is_none());
        assert!(json.get("error").is_none());
        assert_eq!(json["startedAt"], "2026-06-11T00:00:00.000Z");
    }
}
