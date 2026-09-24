//! In-memory [`MetricCollector`] — the Rust port of the TS
//! `InMemoryMetricCollector`.
//!
//! Metrics are appended to a `Vec` behind a [`Mutex`] so the collector is
//! `Send + Sync` and its methods take `&self`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::interfaces::{MetricCollector, MetricInput};
use crate::time::now_iso8601;
use crate::types::Metric;

/// Returns true when `metric_tags` contains every `(key, value)` pair in
/// `query_tags` — the port of the TS `hasTags`.
fn has_tags(metric_tags: &BTreeMap<String, String>, query_tags: &BTreeMap<String, String>) -> bool {
    query_tags
        .iter()
        .all(|(key, value)| metric_tags.get(key) == Some(value))
}

/// An in-memory metric collector that retains every recorded metric.
///
/// Faithful to `@ailu/observability`'s `InMemoryMetricCollector`: `record`
/// stamps the metric with the current time, and `query` filters by exact name
/// plus (optionally) a tag superset match.
#[derive(Default)]
pub struct InMemoryMetricCollector {
    metrics: Mutex<Vec<Metric>>,
}

impl InMemoryMetricCollector {
    /// Create an empty collector.
    pub fn new() -> Self {
        Self::default()
    }
}

impl MetricCollector for InMemoryMetricCollector {
    fn record(&self, metric: MetricInput) {
        let recorded = Metric {
            name: metric.name,
            value: metric.value,
            unit: metric.unit,
            tags: metric.tags,
            timestamp: now_iso8601(),
        };
        self.metrics
            .lock()
            .expect("metric collector mutex poisoned")
            .push(recorded);
    }

    fn query(&self, name: &str, tags: Option<&BTreeMap<String, String>>) -> Vec<Metric> {
        let metrics = self
            .metrics
            .lock()
            .expect("metric collector mutex poisoned");
        metrics
            .iter()
            .filter(|metric| {
                if metric.name != name {
                    return false;
                }
                match tags {
                    None => true,
                    Some(query_tags) => has_tags(&metric.tags, query_tags),
                }
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn records_metrics_with_timestamp() {
        let collector = InMemoryMetricCollector::new();
        collector.record(MetricInput {
            name: "runtime.duration".to_owned(),
            value: 120.0,
            unit: "ms".to_owned(),
            tags: tags(&[("runId", "run-1")]),
        });

        let metrics = collector.query("runtime.duration", None);
        assert_eq!(metrics.len(), 1);
        assert!(!metrics[0].timestamp.is_empty());
    }

    #[test]
    fn queries_by_name_and_tags() {
        let collector = InMemoryMetricCollector::new();
        collector.record(MetricInput {
            name: "runtime.duration".to_owned(),
            value: 120.0,
            unit: "ms".to_owned(),
            tags: tags(&[("runId", "run-1"), ("nodeId", "A")]),
        });
        collector.record(MetricInput {
            name: "runtime.duration".to_owned(),
            value: 220.0,
            unit: "ms".to_owned(),
            tags: tags(&[("runId", "run-2"), ("nodeId", "B")]),
        });
        collector.record(MetricInput {
            name: "tokens.prompt".to_owned(),
            value: 42.0,
            unit: "count".to_owned(),
            tags: tags(&[("runId", "run-1")]),
        });

        let run1_duration = collector.query("runtime.duration", Some(&tags(&[("runId", "run-1")])));
        let run2_duration = collector.query("runtime.duration", Some(&tags(&[("runId", "run-2")])));
        let all_duration = collector.query("runtime.duration", None);

        assert_eq!(run1_duration.len(), 1);
        assert_eq!(run2_duration.len(), 1);
        assert_eq!(all_duration.len(), 2);
    }
}
