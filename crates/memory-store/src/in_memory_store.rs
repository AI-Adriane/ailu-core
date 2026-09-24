//! In-memory [`BaseStore`] — the Rust port of the TS `InMemoryStore`.
//!
//! Entries are kept in insertion order (matching JS `Map` iteration semantics)
//! behind a [`Mutex`] so the store is `Send + Sync` and methods take `&self`.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::interfaces::{BaseStore, MemoryError};
use crate::types::{MemoryItem, MemoryKey, MemoryNamespace};

/// Join a namespace into its canonical string, mirroring the TS `nsToKey`
/// (`namespace.join("|")`).
fn ns_to_key(namespace: &MemoryNamespace) -> String {
    namespace.join("|")
}

/// Build the composite map key `"<ns>:<key>"`, mirroring the TS map keying.
fn map_key(namespace: &MemoryNamespace, key: &str) -> String {
    format!("{}:{}", ns_to_key(namespace), key)
}

/// An in-memory key/value store keyed by namespace + key.
///
/// Faithful to `@ailu/memory-store`'s `InMemoryStore`: `put` preserves the
/// original `createdAt` on overwrite, `search` is a case-insensitive substring
/// match over the stored value's JSON form or the key, and `list` filters by
/// exact namespace and optional key prefix.
#[derive(Default)]
pub struct InMemoryStore {
    /// `(mapKey, item)` pairs, kept in insertion order like a JS `Map`.
    entries: Mutex<Vec<(String, MemoryItem)>>,
}

impl InMemoryStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Current time as an ISO-8601 / RFC-3339 UTC string with millisecond
/// precision, e.g. `2026-06-11T09:21:00.000Z` — matching JS
/// `new Date().toISOString()`.
fn now_iso8601() -> String {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();

    let secs_of_day = total_secs % 86_400;
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    // Convert days-since-epoch to a civil (year, month, day) date using the
    // algorithm from Howard Hinnant's `days_from_civil` inverse.
    let days = (total_secs / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Convert a count of days since 1970-01-01 into a `(year, month, day)` civil
/// date. Port of Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[async_trait]
impl BaseStore for InMemoryStore {
    async fn get(
        &self,
        namespace: &MemoryNamespace,
        key: &MemoryKey,
    ) -> Result<Option<MemoryItem>, MemoryError> {
        let target = map_key(namespace, key);
        let entries = self.entries.lock().expect("memory store mutex poisoned");
        Ok(entries
            .iter()
            .find(|(mk, _)| mk == &target)
            .map(|(_, item)| item.clone()))
    }

    async fn put(
        &self,
        namespace: &MemoryNamespace,
        key: &MemoryKey,
        value: serde_json::Value,
    ) -> Result<MemoryItem, MemoryError> {
        let target = map_key(namespace, key);
        let now = now_iso8601();
        let mut entries = self.entries.lock().expect("memory store mutex poisoned");

        let created_at = entries
            .iter()
            .find(|(mk, _)| mk == &target)
            .map(|(_, item)| item.created_at.clone())
            .unwrap_or_else(|| now.clone());

        let item = MemoryItem {
            namespace: namespace.clone(),
            key: key.clone(),
            value,
            created_at,
            updated_at: now,
            embedding: None,
        };

        match entries.iter_mut().find(|(mk, _)| mk == &target) {
            Some((_, slot)) => *slot = item.clone(),
            None => entries.push((target, item.clone())),
        }
        Ok(item)
    }

    async fn delete(
        &self,
        namespace: &MemoryNamespace,
        key: &MemoryKey,
    ) -> Result<(), MemoryError> {
        let target = map_key(namespace, key);
        let mut entries = self.entries.lock().expect("memory store mutex poisoned");
        entries.retain(|(mk, _)| mk != &target);
        Ok(())
    }

    async fn search(
        &self,
        namespace: &MemoryNamespace,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<MemoryItem>, MemoryError> {
        let q = query.to_lowercase();
        let ns = ns_to_key(namespace);
        let entries = self.entries.lock().expect("memory store mutex poisoned");
        Ok(entries
            .iter()
            .map(|(_, item)| item)
            .filter(|item| ns_to_key(&item.namespace) == ns)
            .filter(|item| {
                item.value.to_string().to_lowercase().contains(&q)
                    || item.key.to_lowercase().contains(&q)
            })
            .take(top_k)
            .cloned()
            .collect())
    }

    async fn list(
        &self,
        namespace: &MemoryNamespace,
        prefix: Option<&str>,
    ) -> Result<Vec<MemoryItem>, MemoryError> {
        let ns = ns_to_key(namespace);
        let entries = self.entries.lock().expect("memory store mutex poisoned");
        Ok(entries
            .iter()
            .map(|(_, item)| item)
            .filter(|item| ns_to_key(&item.namespace) == ns)
            .filter(|item| prefix.is_none_or(|p| item.key.starts_with(p)))
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ns(parts: &[&str]) -> MemoryNamespace {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[tokio::test]
    async fn supports_put_get_and_delete() {
        let store = InMemoryStore::new();
        store
            .put(
                &ns(&["user:u1"]),
                &"profile".to_owned(),
                json!({ "name": "Ada" }),
            )
            .await
            .unwrap();

        let item = store
            .get(&ns(&["user:u1"]), &"profile".to_owned())
            .await
            .unwrap();
        assert_eq!(item.unwrap().value, json!({ "name": "Ada" }));

        store
            .delete(&ns(&["user:u1"]), &"profile".to_owned())
            .await
            .unwrap();
        let after_delete = store
            .get(&ns(&["user:u1"]), &"profile".to_owned())
            .await
            .unwrap();
        assert!(after_delete.is_none());
    }

    #[tokio::test]
    async fn lists_keys_with_prefix() {
        let store = InMemoryStore::new();
        store
            .put(
                &ns(&["agent:risk"]),
                &"memo:1".to_owned(),
                json!({ "risk": "high" }),
            )
            .await
            .unwrap();
        store
            .put(
                &ns(&["agent:risk"]),
                &"memo:2".to_owned(),
                json!({ "risk": "low" }),
            )
            .await
            .unwrap();
        store
            .put(
                &ns(&["agent:risk"]),
                &"note:1".to_owned(),
                json!({ "note": true }),
            )
            .await
            .unwrap();

        let list = store
            .list(&ns(&["agent:risk"]), Some("memo:"))
            .await
            .unwrap();
        let keys: Vec<&str> = list.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["memo:1", "memo:2"]);
    }

    #[tokio::test]
    async fn performs_basic_textual_search() {
        let store = InMemoryStore::new();
        store
            .put(
                &ns(&["agent:risk"]),
                &"memo:1".to_owned(),
                json!({ "summary": "critical supplier risk" }),
            )
            .await
            .unwrap();
        store
            .put(
                &ns(&["agent:risk"]),
                &"memo:2".to_owned(),
                json!({ "summary": "stable account" }),
            )
            .await
            .unwrap();

        let found = store
            .search(&ns(&["agent:risk"]), "critical", 5)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "memo:1");
    }

    #[tokio::test]
    async fn isolates_items_across_namespaces() {
        let store = InMemoryStore::new();
        store
            .put(
                &ns(&["user:u1"]),
                &"profile".to_owned(),
                json!({ "name": "Ada" }),
            )
            .await
            .unwrap();
        store
            .put(
                &ns(&["user:u2"]),
                &"profile".to_owned(),
                json!({ "name": "Bob" }),
            )
            .await
            .unwrap();

        // Same key, different namespace: must not collide.
        let u1 = store
            .get(&ns(&["user:u1"]), &"profile".to_owned())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(u1.value, json!({ "name": "Ada" }));

        // list is scoped to the exact namespace.
        let u2_list = store.list(&ns(&["user:u2"]), None).await.unwrap();
        assert_eq!(u2_list.len(), 1);
        assert_eq!(u2_list[0].value, json!({ "name": "Bob" }));

        // search is scoped too: "Ada" lives only in u1.
        let cross = store.search(&ns(&["user:u2"]), "ada", 5).await.unwrap();
        assert!(cross.is_empty());
    }

    #[tokio::test]
    async fn put_preserves_created_at_and_caps_search_results() {
        let store = InMemoryStore::new();
        let first = store
            .put(&ns(&["agent:x"]), &"k".to_owned(), json!({ "v": 1 }))
            .await
            .unwrap();
        let second = store
            .put(&ns(&["agent:x"]), &"k".to_owned(), json!({ "v": 2 }))
            .await
            .unwrap();

        // Overwrite keeps the original createdAt.
        assert_eq!(first.created_at, second.created_at);
        assert_eq!(second.value, json!({ "v": 2 }));

        // topK caps the number of results.
        store
            .put(
                &ns(&["agent:x"]),
                &"hit:1".to_owned(),
                json!({ "tag": "match" }),
            )
            .await
            .unwrap();
        store
            .put(
                &ns(&["agent:x"]),
                &"hit:2".to_owned(),
                json!({ "tag": "match" }),
            )
            .await
            .unwrap();
        let capped = store.search(&ns(&["agent:x"]), "match", 1).await.unwrap();
        assert_eq!(capped.len(), 1);

        // topK of zero yields nothing.
        let none = store.search(&ns(&["agent:x"]), "match", 0).await.unwrap();
        assert!(none.is_empty());
    }
}
