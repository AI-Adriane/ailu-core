//! Canonical digests used for request fingerprints and approval call keys.
//!
//! Every field is written as `<tag length>:<tag>=<value length>:<value>|`, so no value can
//! be mistaken for a separator or for another field, and the same bytes always give the
//! same digest. Structured values are serialized by `serde_json` from structs (field order is
//! the declaration order), never from maps.

use crate::ids::hex;
use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Incremental canonical digest. Large values (such as `stdin`) are hashed as they are
/// written; they are never copied into a string.
pub(crate) struct Canon(Sha256);

impl Canon {
    /// Start a digest in a named domain, so digests of different kinds never collide.
    pub(crate) fn new(domain: &str) -> Self {
        let mut canon = Self(Sha256::new());
        canon.bytes("domain", domain.as_bytes());
        canon
    }

    pub(crate) fn bytes(&mut self, tag: &str, value: &[u8]) -> &mut Self {
        self.0.update(tag.len().to_string().as_bytes());
        self.0.update(b":");
        self.0.update(tag.as_bytes());
        self.0.update(b"=");
        self.0.update(value.len().to_string().as_bytes());
        self.0.update(b":");
        self.0.update(value);
        self.0.update(b"|");
        self
    }

    pub(crate) fn str(&mut self, tag: &str, value: &str) -> &mut Self {
        self.bytes(tag, value.as_bytes())
    }

    pub(crate) fn num(&mut self, tag: &str, value: u64) -> &mut Self {
        self.bytes(tag, value.to_string().as_bytes())
    }

    /// Serialize a plain struct (never a map) to JSON and add it. Serialization of the
    /// request types cannot fail; if it ever did, an explicit marker keeps the digest
    /// distinct from every valid one instead of panicking.
    pub(crate) fn json<T: serde::Serialize>(&mut self, tag: &str, value: &T) -> &mut Self {
        match serde_json::to_vec(value) {
            Ok(bytes) => self.bytes(tag, &bytes),
            Err(_) => self.bytes(tag, b"\0unserializable"),
        }
    }

    pub(crate) fn finish(self) -> String {
        hex(&self.0.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(build: impl FnOnce(&mut Canon)) -> String {
        let mut canon = Canon::new("test");
        build(&mut canon);
        canon.finish()
    }

    #[test]
    fn same_input_same_digest() {
        let a = digest(|c| {
            c.str("a", "x").num("n", 3);
        });
        let b = digest(|c| {
            c.str("a", "x").num("n", 3);
        });
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn field_boundaries_cannot_be_shifted() {
        let a = digest(|c| {
            c.str("a", "bc").str("d", "e");
        });
        let b = digest(|c| {
            c.str("a", "b").str("cd", "e");
        });
        let c = digest(|c| {
            c.str("a", "b|1:c=1:d").str("e", "f");
        });
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn domains_separate_digests() {
        let mut a = Canon::new("one");
        let mut b = Canon::new("two");
        a.str("k", "v");
        b.str("k", "v");
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn sha256_hex_matches_the_known_answer() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
