//! Test-only journal work measurement. Production keeps the standard `HashMap` unchanged.
//!
//! Charge one unit for an indexed lookup, or one per journal entry visited by an iterator.
//! Counting randomized hash-table bucket probes would make the bound depend on the hash seed;
//! this measures the journal-level rescan amplification instead. Keeping iteration measured
//! means a regression from `get_mut` to a full entry scan cannot hide behind equivalent results.

use std::cell::Cell;
use std::collections::{hash_map::Entry, HashMap};
use std::hash::Hash;

pub(crate) struct MeasuredMap<K, V> {
    inner: HashMap<K, V>,
    work: Cell<usize>,
}

impl<K: Eq + Hash, V> MeasuredMap<K, V> {
    pub(crate) fn new() -> Self {
        Self {
            inner: HashMap::new(),
            work: Cell::new(0),
        }
    }

    pub(crate) fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        self.inner.entry(key)
    }

    pub(crate) fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.work.set(self.work.get() + 1);
        self.inner.get_mut(key)
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        let work = &self.work;
        self.inner.iter_mut().inspect(move |_| {
            work.set(work.get() + 1);
        })
    }

    pub(crate) fn lookup_work(&self) -> usize {
        self.work.get()
    }
}

#[test]
fn a_full_scan_exceeds_the_linear_work_budget() {
    let count = 256;
    let mut entries = MeasuredMap::new();
    for key in 0..count {
        entries.entry(key).or_insert(());
    }
    // Negative control: the same repeated-miss workload used by the journal tests, with the
    // original full-scan algorithm. The counter must observe every visited entry.
    for _ in 0..count {
        assert!(!entries.iter_mut().any(|(key, _)| *key == count));
    }
    assert_eq!(entries.lookup_work(), count * count);
    assert!(entries.lookup_work() > 4 * count);
}
