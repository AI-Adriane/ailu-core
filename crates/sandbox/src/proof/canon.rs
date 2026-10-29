//! The canonical form of a proof and the two hashes built on it.
//!
//! These are the primitives of the approval attestation chain, written again here so that the
//! sandbox chain does not depend on the approval engine: JSON with object keys sorted in UTF-16
//! code unit order (what JavaScript's `Array.prototype.sort` compares) and numbers as
//! `JSON.stringify` writes them, hashed with SHA-256 and chained as
//! `sha256(<prev payload hash or "">:<payload hash>)`. The tests of the crate check that this
//! module and the approval engine write the same bytes for the values a proof holds.
//!
//! A proof holds only strings, booleans, `null` and non-negative integers. An integer above
//! 2^53 - 1 cannot be written the way JavaScript reads it, and a fractional number is never
//! part of a proof: both make the canonical form fail, and a record that has none cannot be
//! signed or verified.

use super::ExecutionProof;
use crate::digest::sha256_hex;
use serde_json::Value;

/// Largest integer JavaScript reads back unchanged.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Deterministic JSON of `value`, or `None` when it holds a number that has no single
/// JavaScript spelling (a fraction, or an integer above 2^53 - 1 in magnitude).
pub fn canonical_json(value: &Value) -> Option<String> {
    let mut out = String::new();
    write(value, &mut out)?;
    Some(out)
}

fn write(value: &Value, out: &mut String) -> Option<()> {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write(map.get(key)?, out)?;
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write(item, out)?;
            }
            out.push(']');
        }
        Value::Number(number) => {
            if let Some(n) = number.as_u64() {
                if n > MAX_SAFE_INTEGER {
                    return None;
                }
                out.push_str(&n.to_string());
            } else if let Some(n) = number.as_i64() {
                if n.unsigned_abs() > MAX_SAFE_INTEGER {
                    return None;
                }
                out.push_str(&n.to_string());
            } else {
                return None;
            }
        }
        other => out.push_str(&other.to_string()),
    }
    Some(())
}

/// The canonical JSON of a proof: the exact text that [`payload_hash`] hashes.
pub fn canonical_proof_json(proof: &ExecutionProof) -> Option<String> {
    canonical_json(&serde_json::to_value(proof).ok()?)
}

/// Lowercase hex SHA-256 of the canonical JSON of `proof`: the `payload_hash` of its record.
/// `kind` is part of the proof, so it is part of this hash.
pub fn payload_hash(proof: &ExecutionProof) -> Option<String> {
    canonical_proof_json(proof).map(|text| sha256_hex(text.as_bytes()))
}

/// The hash a signature covers: the payload hash chained to the previous one
/// (`sha256("<prev>:<payload>")`, the previous hash being empty for the first record).
pub fn chain_hash(payload_hash: &str, prev_hash: Option<&str>) -> String {
    sha256_hex(format!("{}:{}", prev_hash.unwrap_or(""), payload_hash).as_bytes())
}

/// The bytes a proof signature covers: [`PROOF_SIGN_DOMAIN`](super::PROOF_SIGN_DOMAIN), then the
/// [`chain_hash`] of the record as lowercase hex text.
pub fn signing_message(payload_hash: &str, prev_hash: Option<&str>) -> Vec<u8> {
    let mut message = super::PROOF_SIGN_DOMAIN.to_vec();
    message.extend_from_slice(chain_hash(payload_hash, prev_hash).as_bytes());
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_are_sorted_in_utf16_order_at_every_depth() {
        let value =
            json!({"b": 1, "a": {"z": [true, null], "y": "x"}, "\u{1F600}": 2, "\u{FF5E}": 3});
        // U+1F600 is the surrogate pair 0xD83D 0xDE00 in UTF-16, which sorts before U+FF5E
        // (0xFF5E), although its code point is larger: JavaScript's order, not byte order.
        assert_eq!(
            canonical_json(&value).unwrap(),
            "{\"a\":{\"y\":\"x\",\"z\":[true,null]},\"b\":1,\"\u{1F600}\":2,\"\u{FF5E}\":3}"
        );
    }

    #[test]
    fn integers_above_two_to_the_53_and_fractions_have_no_canonical_form() {
        assert_eq!(
            canonical_json(&json!(9_007_199_254_740_991u64)).unwrap(),
            "9007199254740991"
        );
        assert!(canonical_json(&json!(9_007_199_254_740_992u64)).is_none());
        assert!(canonical_json(&json!(-9_007_199_254_740_992i64)).is_none());
        assert!(canonical_json(&json!(1.5)).is_none());
        assert!(canonical_json(&json!({"deep": [{"n": 1.5}]})).is_none());
    }

    #[test]
    fn chain_hash_is_sha256_of_prev_colon_payload() {
        assert_eq!(chain_hash("p", None), sha256_hex(b":p"));
        assert_eq!(chain_hash("p", Some("q")), sha256_hex(b"q:p"));
        // As in the approval chain, an empty previous hash and none hash alike: the chain
        // verifier compares the link itself, so `Some("")` is still not a first record.
        assert_eq!(chain_hash("p", None), chain_hash("p", Some("")));
    }
}
