//! The virtual filesystem of one simulated sandbox.

use crate::collected::glob_match;
use crate::path::LogicalPath;
use ailu_fs_backend::normalize_path;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// Why a write to the virtual filesystem failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SimFsError {
    /// The path is not a valid logical path (same rule as the virtual filesystem of the
    /// engine: no `..`, no absolute path, no backslash, no NUL).
    #[error("invalid path")]
    InvalidPath,
    /// The write would take the sandbox past its ephemeral disk limit. Nothing was written.
    #[error("disk full")]
    DiskFull,
}

/// The files of one sandbox, in memory, under a disk quota.
///
/// A program reads and writes through it with shared references (it is handed to the program
/// inside a [`crate::sim::SimInvocation`]). A write past the quota is refused and remembered:
/// even if the program ignores the error, the simulator reports the sandbox as having
/// exceeded its disk limit.
///
/// `Debug` shows only counts.
pub struct SimFs {
    files: RefCell<BTreeMap<String, Vec<u8>>>,
    capacity: u64,
    used: Cell<u64>,
    exhausted: Cell<bool>,
}

impl SimFs {
    /// An empty filesystem that holds at most `capacity_bytes` in total.
    pub fn new(capacity_bytes: u64) -> Self {
        Self::from_files(BTreeMap::new(), capacity_bytes)
    }

    /// A filesystem holding `files`, under a quota of `capacity` bytes.
    pub fn from_files(files: BTreeMap<String, Vec<u8>>, capacity: u64) -> Self {
        let used = files.values().map(|bytes| bytes.len() as u64).sum();
        Self {
            files: RefCell::new(files),
            capacity,
            used: Cell::new(used),
            exhausted: Cell::new(false),
        }
    }

    /// Create or replace a file.
    pub fn write(&self, path: &str, bytes: impl Into<Vec<u8>>) -> Result<(), SimFsError> {
        let path = LogicalPath::new(path).map_err(|_| SimFsError::InvalidPath)?;
        let bytes = bytes.into();
        let mut files = self.files.borrow_mut();
        let old = files.get(path.as_str()).map_or(0, |b| b.len() as u64);
        let used = self
            .used
            .get()
            .saturating_sub(old)
            .saturating_add(bytes.len() as u64);
        if used > self.capacity {
            self.exhausted.set(true);
            return Err(SimFsError::DiskFull);
        }
        self.used.set(used);
        files.insert(path.as_str().to_owned(), bytes);
        Ok(())
    }

    /// The content of a file, if it exists.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let path = LogicalPath::new(path).ok()?;
        self.files.borrow().get(path.as_str()).cloned()
    }

    /// Whether a file exists.
    pub fn exists(&self, path: &str) -> bool {
        LogicalPath::new(path)
            .ok()
            .is_some_and(|p| self.files.borrow().contains_key(p.as_str()))
    }

    /// Delete a file; `true` if it existed.
    pub fn remove(&self, path: &str) -> bool {
        let Ok(path) = LogicalPath::new(path) else {
            return false;
        };
        match self.files.borrow_mut().remove(path.as_str()) {
            Some(bytes) => {
                self.used
                    .set(self.used.get().saturating_sub(bytes.len() as u64));
                true
            }
            None => false,
        }
    }

    /// Every path, sorted.
    pub fn list(&self) -> Vec<String> {
        self.files.borrow().keys().cloned().collect()
    }

    /// Bytes currently stored.
    pub fn used(&self) -> u64 {
        self.used.get()
    }

    /// Whether a write was refused because of the quota.
    pub fn exhausted(&self) -> bool {
        self.exhausted.get()
    }

    /// The files whose path matches at least one of `patterns`, sorted by path.
    pub fn matching(&self, patterns: &[String]) -> Vec<(String, Vec<u8>)> {
        let patterns: Vec<String> = patterns
            .iter()
            .map(|p| normalize_path(p).unwrap_or_else(|_| p.clone()))
            .collect();
        self.files
            .borrow()
            .iter()
            .filter(|(path, _)| patterns.iter().any(|p| glob_match(p, path)))
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect()
    }

    /// Give the files back.
    pub fn into_files(self) -> BTreeMap<String, Vec<u8>> {
        self.files.into_inner()
    }
}

impl std::fmt::Debug for SimFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimFs")
            .field("files", &self.files.borrow().len())
            .field("used", &self.used.get())
            .field("capacity", &self.capacity)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reads_and_replaces_under_a_quota() {
        let fs = SimFs::new(10);
        fs.write("a/b.txt", "hello").unwrap();
        assert_eq!(fs.read("a/b.txt").unwrap(), b"hello");
        assert_eq!(fs.used(), 5);
        fs.write("a/b.txt", "hi").unwrap();
        assert_eq!(fs.used(), 2, "a replaced file frees its old size");
        fs.write("c", vec![0; 8]).unwrap();
        assert_eq!(fs.write("d", "x"), Err(SimFsError::DiskFull));
        assert!(fs.exhausted());
        assert!(!fs.exists("d"), "a refused write leaves nothing");
        assert_eq!(fs.list(), vec!["a/b.txt".to_owned(), "c".to_owned()]);
        assert!(fs.remove("c"));
        assert!(!fs.remove("c"));
        assert_eq!(fs.used(), 2);
    }

    #[test]
    fn paths_follow_the_logical_path_rule() {
        let fs = SimFs::new(100);
        for bad in ["../x", "/abs", "a\\b", "a\0b", ""] {
            assert_eq!(fs.write(bad, "x"), Err(SimFsError::InvalidPath), "{bad:?}");
            assert_eq!(fs.read(bad), None);
        }
        fs.write("work//out.txt", "x").unwrap();
        assert!(fs.exists("work/out.txt"));
    }

    #[test]
    fn matching_uses_the_collect_patterns() {
        let fs = SimFs::new(100);
        fs.write("out/a.txt", "1").unwrap();
        fs.write("out/sub/b.txt", "2").unwrap();
        fs.write("tmp/c.txt", "3").unwrap();
        let shallow = fs.matching(&["out/*".to_owned()]);
        assert_eq!(shallow.len(), 1);
        let deep = fs.matching(&["out/**".to_owned()]);
        assert_eq!(deep.len(), 2);
    }

    #[test]
    fn debug_shows_no_content_or_names() {
        let fs = SimFs::new(100);
        fs.write("secret-name", "s3cr3t").unwrap();
        let shown = format!("{fs:?}");
        assert!(!shown.contains("s3cr3t") && !shown.contains("secret-name"));
    }
}
