//! What `collect` returns: the raw bytes a backend hands over, and the redacted result the
//! guard derives from them.

use crate::digest::{sha256_hex, Canon};
use crate::error::SandboxError;
use crate::path::LogicalPath;
use crate::request::CollectRequest;
use crate::secret::ResolvedSecret;
use serde::{Deserialize, Serialize};

/// Replacement written in place of a known secret value.
pub const REDACTION_MARKER: &str = "[REDACTED]";

/// The secret values a backend resolved for one sandbox, handed back to the guard so that it
/// can redact them from the output.
///
/// It has no `Debug`, no `Display`, no `Clone` and no `Serialize`. The guard reads it only to
/// redact and drops it right after, which wipes the memory.
///
/// ```compile_fail
/// fn needs_debug<T: std::fmt::Debug>() {}
/// needs_debug::<ailu_sandbox::ResolvedSecrets>();
/// ```
///
/// ```compile_fail
/// fn needs_serialize<T: serde::Serialize>() {}
/// needs_serialize::<ailu_sandbox::ResolvedSecrets>();
/// ```
#[derive(Default)]
pub struct ResolvedSecrets(Vec<ResolvedSecret>);

impl ResolvedSecrets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one resolved value.
    pub fn push(&mut self, secret: ResolvedSecret) {
        self.0.push(secret);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn values(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(ResolvedSecret::expose)
    }
}

/// One output file as delivered by the backend.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawFile {
    pub path: LogicalPath,
    pub bytes: Vec<u8>,
    /// Lowercase hex SHA-256 the backend declares for `bytes`. The guard recomputes it.
    pub declared_sha256: String,
}

/// What a backend returns from `collect`.
///
/// The serializable part (bytes and declared values) is what a backend keeps, bounded, for
/// the retention. `secrets` is never serialized: after deserialization it is empty.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawCollected {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub declared_stdout_sha256: String,
    pub declared_stderr_sha256: String,
    #[serde(default)]
    pub files: Vec<RawFile>,
    /// The backend cut the collection at a bound.
    #[serde(default)]
    pub declared_truncated: bool,
    /// Secret values resolved for this sandbox, for redaction only.
    #[serde(skip)]
    pub secrets: ResolvedSecrets,
}

impl std::fmt::Debug for RawCollected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawCollected")
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr_bytes", &self.stderr.len())
            .field("files", &self.files.len())
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

/// One collected file, redacted and with the digest the guard computed.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedFile {
    pub path: LogicalPath,
    pub size: u64,
    /// SHA-256 of `bytes` (after redaction).
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// The redacted result of `collect`, with digests the guard computed itself.
///
/// Until the run's artifact store is wired in, the redacted bytes travel inside this value.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collected {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub files: Vec<CollectedFile>,
    pub truncated: bool,
    /// SHA-256 over the sorted `(path, sha256)` pairs of the files.
    pub outputs_digest: String,
}

impl std::fmt::Debug for Collected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collected")
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr_bytes", &self.stderr.len())
            .field("files", &self.files.len())
            .field("truncated", &self.truncated)
            .field("outputs_digest", &self.outputs_digest)
            .finish()
    }
}

fn refuse(message: &str) -> SandboxError {
    SandboxError::Backend(message.to_owned())
}

/// Replace every occurrence of every non-empty secret value with [`REDACTION_MARKER`]. Longer
/// values are replaced first so that a value containing another one is fully hidden. This
/// only catches values the program did not transform (encoded, split, hashed…).
pub(crate) fn redact(bytes: &[u8], secrets: &ResolvedSecrets) -> Vec<u8> {
    let mut needles: Vec<&[u8]> = secrets
        .values()
        .filter(|v| !v.is_empty())
        .map(str::as_bytes)
        .collect();
    needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    'scan: while i < bytes.len() {
        for needle in &needles {
            if bytes[i..].starts_with(needle) {
                out.extend_from_slice(REDACTION_MARKER.as_bytes());
                i += needle.len();
                continue 'scan;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Check what a backend delivered against the bounds the guard knows, recompute the digests,
/// redact, and build [`Collected`]. The secrets are dropped (and wiped) on return.
///
/// A delivery over a bound, with a duplicated path, or whose declared digest differs from its
/// bytes is refused with a fixed message that never echoes content.
pub(crate) fn derive(
    raw: RawCollected,
    output_cap: u64,
    request: &CollectRequest,
) -> Result<Collected, SandboxError> {
    let RawCollected {
        stdout,
        stderr,
        declared_stdout_sha256,
        declared_stderr_sha256,
        files,
        declared_truncated,
        secrets,
    } = raw;
    if stdout.len() as u64 > output_cap || stderr.len() as u64 > output_cap {
        return Err(refuse("collected stream exceeds its bound"));
    }
    if files.len() as u64 > u64::from(request.max_files) {
        return Err(refuse("collected files exceed their bound"));
    }
    let total: u64 = files.iter().map(|f| f.bytes.len() as u64).sum();
    if total > request.max_bytes {
        return Err(refuse("collected bytes exceed their bound"));
    }
    let matches = |declared: &str, bytes: &[u8]| declared.eq_ignore_ascii_case(&sha256_hex(bytes));
    if !matches(&declared_stdout_sha256, &stdout)
        || !matches(&declared_stderr_sha256, &stderr)
        || files.iter().any(|f| !matches(&f.declared_sha256, &f.bytes))
    {
        return Err(refuse("declared digest does not match delivered bytes"));
    }
    let mut seen = std::collections::HashSet::new();
    if files.iter().any(|f| !seen.insert(f.path.as_str())) {
        return Err(refuse("duplicate collected path"));
    }

    let stdout = redact(&stdout, &secrets);
    let stderr = redact(&stderr, &secrets);
    let mut collected_files: Vec<CollectedFile> = files
        .iter()
        .map(|f| {
            let bytes = redact(&f.bytes, &secrets);
            CollectedFile {
                path: f.path.clone(),
                size: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
                bytes,
            }
        })
        .collect();
    collected_files.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    let mut canon = Canon::new("sandbox.outputs.v1");
    for file in &collected_files {
        canon.str("path", file.path.as_str());
        canon.str("sha256", &file.sha256);
    }
    Ok(Collected {
        stdout_sha256: sha256_hex(&stdout),
        stderr_sha256: sha256_hex(&stderr),
        stdout,
        stderr,
        files: collected_files,
        truncated: declared_truncated,
        outputs_digest: canon.finish(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CollectRequest {
        CollectRequest {
            patterns: vec!["out/*".to_owned()],
            max_files: 4,
            max_bytes: 1024,
        }
    }

    fn raw(stdout: &[u8], stderr: &[u8]) -> RawCollected {
        RawCollected {
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
            declared_stdout_sha256: sha256_hex(stdout),
            declared_stderr_sha256: sha256_hex(stderr),
            ..RawCollected::default()
        }
    }

    fn file(path: &str, bytes: &[u8]) -> RawFile {
        RawFile {
            path: LogicalPath::new(path).unwrap(),
            bytes: bytes.to_vec(),
            declared_sha256: sha256_hex(bytes),
        }
    }

    #[test]
    fn redacts_known_secret_values_before_digesting() {
        let mut r = raw(b"token=hunter2 ok", b"");
        r.secrets.push(ResolvedSecret::new("hunter2"));
        r.files.push(file("out/a.txt", b"hunter2!hunter2"));
        let c = derive(r, 1024, &request()).unwrap();
        assert_eq!(c.stdout, b"token=[REDACTED] ok");
        assert_eq!(c.files[0].bytes, b"[REDACTED]![REDACTED]");
        assert_eq!(c.stdout_sha256, sha256_hex(&c.stdout));
        assert_eq!(c.files[0].sha256, sha256_hex(&c.files[0].bytes));
        assert_eq!(c.files[0].size, c.files[0].bytes.len() as u64);
    }

    #[test]
    fn longer_secret_wins_over_a_contained_one() {
        let mut secrets = ResolvedSecrets::new();
        secrets.push(ResolvedSecret::new("abc"));
        secrets.push(ResolvedSecret::new("abcdef"));
        secrets.push(ResolvedSecret::new(""));
        assert_eq!(
            redact(b"x abcdef y abc", &secrets),
            b"x [REDACTED] y [REDACTED]"
        );
    }

    #[test]
    fn digest_mismatch_is_refused_without_echoing_content() {
        let mut r = raw(b"payload-xyz", b"");
        r.declared_stdout_sha256 = sha256_hex(b"something else");
        let err = derive(r, 1024, &request()).unwrap_err();
        assert_eq!(err.code(), "backend_error");
        assert!(!err.to_string().contains("payload-xyz"));
        let mut r = raw(b"", b"");
        r.files.push(RawFile {
            declared_sha256: "0".repeat(64),
            ..file("out/a", b"abc")
        });
        assert_eq!(
            derive(r, 1024, &request()).unwrap_err().code(),
            "backend_error"
        );
    }

    #[test]
    fn deliveries_over_a_bound_are_refused() {
        assert!(derive(raw(&[0; 11], b""), 10, &request()).is_err());
        assert!(derive(raw(b"", &[0; 11]), 10, &request()).is_err());
        let mut many = raw(b"", b"");
        for i in 0..5 {
            many.files.push(file(&format!("out/{i}"), b"x"));
        }
        assert!(derive(many, 1024, &request()).is_err());
        let mut big = raw(b"", b"");
        big.files.push(file("out/big", &[1; 1025]));
        assert!(derive(big, 1024, &request()).is_err());
    }

    #[test]
    fn duplicate_paths_are_refused() {
        let mut r = raw(b"", b"");
        r.files.push(file("out/a", b"1"));
        r.files.push(file("out/a", b"2"));
        assert!(derive(r, 1024, &request()).is_err());
    }

    #[test]
    fn outputs_digest_is_order_independent_and_content_sensitive() {
        let build = |files: Vec<RawFile>| {
            let mut r = raw(b"", b"");
            r.files = files;
            derive(r, 1024, &request()).unwrap().outputs_digest
        };
        let ab = build(vec![file("out/a", b"1"), file("out/b", b"2")]);
        let ba = build(vec![file("out/b", b"2"), file("out/a", b"1")]);
        let changed = build(vec![file("out/a", b"1"), file("out/b", b"3")]);
        assert_eq!(ab, ba);
        assert_ne!(ab, changed);
    }

    #[test]
    fn raw_collected_round_trips_without_secrets() {
        let mut r = raw(b"out", b"");
        r.secrets.push(ResolvedSecret::new("hunter2"));
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("hunter2") && !json.contains("secrets"));
        let back: RawCollected = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stdout, b"out");
        assert!(back.secrets.is_empty());
        let extra = json.replacen('{', "{\"tenant\":\"x\",", 1);
        assert!(serde_json::from_str::<RawCollected>(&extra).is_err());
    }

    #[test]
    fn debug_shows_sizes_never_bytes() {
        let mut r = raw(b"s3cr3t-out", b"s3cr3t-err");
        r.files.push(file("out/a", b"s3cr3t-file"));
        r.secrets.push(ResolvedSecret::new("s3cr3t"));
        assert!(!format!("{r:?}").contains("s3cr3t"));
        let c = derive(r, 1024, &request()).unwrap();
        assert!(!format!("{c:?}").contains("s3cr3t"));
        assert!(!format!("{c:#?}").contains("out/a"));
    }
}
