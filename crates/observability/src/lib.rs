//! Rust port of `@ailu/observability`.
//!
//! Provides the observability value types ([`Span`], [`Metric`],
//! [`ObservabilityEvent`]), the [`Tracer`] / [`MetricCollector`] /
//! [`ObservabilityBus`] seams, and their in-memory implementations.

#![forbid(unsafe_code)]

pub mod in_memory_metric_collector;
pub mod in_memory_observability_bus;
pub mod in_memory_tracer;
pub mod interfaces;
mod time;
pub mod types;

pub use in_memory_metric_collector::InMemoryMetricCollector;
pub use in_memory_observability_bus::InMemoryObservabilityBus;
pub use in_memory_tracer::InMemoryTracer;
pub use interfaces::{
    EventHandler, MetricCollector, MetricInput, ObservabilityBus, Tracer, Unsubscribe,
};
pub use types::{Metric, ObservabilityEvent, Span, SpanId, SpanStatus, TraceId};
