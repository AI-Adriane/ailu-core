//! What `collect` returns: the raw bytes a backend hands over, and the redacted result the
//! guard derives from them.

use crate::digest::{sha256_hex, Canon};
use crate::error::SandboxError;
use crate::path::LogicalPath;
use crate::request::CollectRequest;
use crate::secret::ResolvedSecret;
use ailu_fs_backend::normalize_path;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

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

// ---- redaction (by known value) -------------------------------------------------------

fn hex_encode(bytes: &[u8], upper: bool) -> Vec<u8> {
    let digits: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(digits[(b >> 4) as usize]);
        out.push(digits[(b & 0x0f) as usize]);
    }
    out
}

fn base64_encode(bytes: &[u8], url_safe: bool, pad: bool) -> Vec<u8> {
    let table: &[u8; 64] = if url_safe {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
    } else {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    };
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        let symbols = chunk.len() + 1;
        for i in 0..4 {
            if i < symbols {
                out.push(table[((n >> (18 - 6 * i)) & 63) as usize]);
            } else if pad {
                out.push(b'=');
            }
        }
    }
    out
}

/// Every byte as `%XX`, unreserved ones included.
fn percent_all(bytes: &[u8], upper: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 3);
    for &b in bytes {
        out.push(b'%');
        out.extend(hex_encode(&[b], upper));
    }
    out
}

fn percent_encode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b);
        } else {
            out.push(b'%');
            out.extend(hex_encode(&[b], true));
        }
    }
    out
}

/// Every form of the known secret values that is searched for (held in memory that is wiped
/// when dropped, like the values themselves): the value itself and its
/// hex, base64 (standard and URL-safe, padded or not) and percent-encoded forms. A form of a
/// substring, or a value that was split or hashed, is out of reach: redaction is a safeguard,
/// not a guarantee.
fn needles(secrets: &ResolvedSecrets) -> Vec<Zeroizing<Vec<u8>>> {
    let mut all: Vec<Zeroizing<Vec<u8>>> = Vec::new();
    for value in secrets.values().filter(|v| !v.is_empty()) {
        let raw = value.as_bytes();
        all.push(Zeroizing::new(raw.to_vec()));
        all.push(Zeroizing::new(hex_encode(raw, false)));
        all.push(Zeroizing::new(hex_encode(raw, true)));
        for url_safe in [false, true] {
            for pad in [false, true] {
                all.push(Zeroizing::new(base64_encode(raw, url_safe, pad)));
            }
        }
        all.push(Zeroizing::new(percent_encode(raw)));
        // Every byte percent-encoded, with upper- and lower-case hex digits.
        all.push(Zeroizing::new(percent_all(raw, true)));
        all.push(Zeroizing::new(percent_all(raw, false)));
    }
    all.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
    all.dedup_by(|a, b| a.as_slice() == b.as_slice());
    all
}

/// Redact a string by known secret value: occurrences of the values the host knows (and of
/// their encoded forms) are replaced. This is redaction by known value, not the pattern-based
/// redaction of ADR 0032: a secret the host does not know is not recognized.
pub(crate) fn redact_text(text: &str, secrets: &ResolvedSecrets) -> String {
    String::from_utf8_lossy(&redact_with(text.as_bytes(), &needles(secrets))).into_owned()
}

/// Replace every occurrence of every known secret form with [`REDACTION_MARKER`]. Overlapping
/// or touching occurrences (`abcd` and `cdef` in `abcdef`) are merged into one interval
/// first, so no fragment of a value survives between two replacements.
#[cfg(test)]
fn redact(bytes: &[u8], secrets: &ResolvedSecrets) -> Vec<u8> {
    redact_with(bytes, &needles(secrets))
}

fn redact_with(bytes: &[u8], needles: &[Zeroizing<Vec<u8>>]) -> Vec<u8> {
    // The occurrences are merged as they are found: only the interval being built is kept,
    // so the memory used is bounded by the delivered bytes however many occurrences (and
    // overlapping secret forms) there are.
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    let mut open: Option<(usize, usize)> = None;
    let close = |out: &mut Vec<u8>, at: &mut usize, span: Option<(usize, usize)>| {
        if let Some((start, end)) = span {
            out.extend_from_slice(&bytes[*at..start]);
            out.extend_from_slice(REDACTION_MARKER.as_bytes());
            *at = end;
        }
    };
    for start in 0..bytes.len() {
        let rest = &bytes[start..];
        let longest = needles
            .iter()
            .filter(|needle| !needle.is_empty() && rest.starts_with(needle.as_slice()))
            .map(|needle| start + needle.len())
            .max();
        let Some(end) = longest else { continue };
        match open {
            Some((first, last)) if start <= last => open = Some((first, last.max(end))),
            _ => {
                close(&mut out, &mut at, open);
                open = Some((start, end));
            }
        }
    }
    close(&mut out, &mut at, open);
    out.extend_from_slice(&bytes[at..]);
    out
}

// ---- requested patterns -----------------------------------------------------------------

/// Match `path` against a collect pattern: `*` matches any run of characters except `/`,
/// `**` any run including `/`, `?` one character other than `/`; anything else is literal.
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum Tok {
        Lit(char),
        One,
        Star,
        Deep,
    }
    let mut toks = Vec::new();
    let p: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            '*' if p.get(i + 1) == Some(&'*') => {
                toks.push(Tok::Deep);
                i += 2;
                while p.get(i) == Some(&'*') {
                    i += 1;
                }
            }
            '*' => {
                toks.push(Tok::Star);
                i += 1;
            }
            '?' => {
                toks.push(Tok::One);
                i += 1;
            }
            c => {
                toks.push(Tok::Lit(c));
                i += 1;
            }
        }
    }
    // reach[j]: the first j tokens match the text consumed so far.
    let mut reach = vec![false; toks.len() + 1];
    reach[0] = true;
    for (j, tok) in toks.iter().enumerate() {
        if matches!(tok, Tok::Star | Tok::Deep) && reach[j] {
            reach[j + 1] = true;
        }
    }
    for c in path.chars() {
        let mut next = vec![false; toks.len() + 1];
        for (j, tok) in toks.iter().enumerate() {
            next[j + 1] = match tok {
                Tok::Lit(l) => reach[j] && *l == c,
                Tok::One => reach[j] && c != '/',
                Tok::Star => (reach[j] || reach[j + 1]) && c != '/',
                Tok::Deep => reach[j] || reach[j + 1],
            };
            if matches!(tok, Tok::Star | Tok::Deep) && next[j] {
                next[j + 1] = true;
            }
        }
        reach = next;
    }
    reach[toks.len()]
}

/// Check what a backend delivered against the bounds the guard knows, recompute the digests,
/// redact, and build [`Collected`]. The secrets are dropped (and wiped) on return.
///
/// `granted_secrets` is how many secret grants the sandbox was created with. A backend that
/// hands back fewer resolved values than that cannot be redacted against, so the delivery is
/// refused rather than returned unredacted. Every delivered file must match a requested
/// pattern. A delivery over a bound, with a duplicated path, or whose declared digest differs
/// from its bytes is refused with a fixed message that never echoes content. File names are
/// redacted too.
pub(crate) fn derive(
    raw: RawCollected,
    output_cap: u64,
    request: &CollectRequest,
    granted_secrets: usize,
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
    if secrets.len() < granted_secrets {
        return Err(refuse("fewer secrets resolved than granted"));
    }
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
    let patterns: Vec<String> = request
        .patterns
        .iter()
        .map(|p| normalize_path(p).unwrap_or_else(|_| p.clone()))
        .collect();
    if files
        .iter()
        .any(|f| !patterns.iter().any(|p| glob_match(p, f.path.as_str())))
    {
        return Err(refuse("collected path outside the requested patterns"));
    }
    let mut seen = std::collections::HashSet::new();
    if files.iter().any(|f| !seen.insert(f.path.as_str())) {
        return Err(refuse("duplicate collected path"));
    }

    let forms = needles(&secrets);
    let stdout = redact_with(&stdout, &forms);
    let stderr = redact_with(&stderr, &forms);
    let mut collected_files = Vec::with_capacity(files.len());
    for f in &files {
        let bytes = redact_with(&f.bytes, &forms);
        let redacted_name = redact_with(f.path.as_str().as_bytes(), &forms);
        let path = String::from_utf8(redacted_name)
            .ok()
            .and_then(|name| LogicalPath::new(&name).ok())
            .ok_or_else(|| refuse("collected path cannot be redacted"))?;
        collected_files.push(CollectedFile {
            path,
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            bytes,
        });
    }
    collected_files.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    if collected_files
        .windows(2)
        .any(|w| w[0].path.as_str() == w[1].path.as_str())
    {
        return Err(refuse("duplicate collected path"));
    }
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
        let c = derive(r, 1024, &request(), 0).unwrap();
        assert_eq!(c.stdout, b"token=[REDACTED] ok");
        assert_eq!(c.files[0].bytes, b"[REDACTED]![REDACTED]");
        assert_eq!(c.stdout_sha256, sha256_hex(&c.stdout));
        assert_eq!(c.files[0].sha256, sha256_hex(&c.files[0].bytes));
        assert_eq!(c.files[0].size, c.files[0].bytes.len() as u64);
    }

    #[test]
    fn repetitive_output_with_overlapping_secrets_collapses_to_one_marker() {
        // Many secret forms that all match everywhere in a repetitive output: the occurrences
        // are merged as they are found, and the result is one marker.
        let mut secrets = ResolvedSecrets::new();
        for n in 1..=16 {
            secrets.push(ResolvedSecret::new("a".repeat(n)));
        }
        let output = vec![b'a'; 100_000];
        assert_eq!(redact(&output, &secrets), REDACTION_MARKER.as_bytes());
        // A byte that is not part of any occurrence survives between two markers.
        let mut spaced = output.clone();
        spaced.push(b'-');
        spaced.extend_from_slice(&output);
        let expected = [
            REDACTION_MARKER.as_bytes(),
            b"-".as_slice(),
            REDACTION_MARKER.as_bytes(),
        ]
        .concat();
        assert_eq!(redact(&spaced, &secrets), expected);
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
        let err = derive(r, 1024, &request(), 0).unwrap_err();
        assert_eq!(err.code(), "backend_error");
        assert!(!err.to_string().contains("payload-xyz"));
        let mut r = raw(b"", b"");
        r.files.push(RawFile {
            declared_sha256: "0".repeat(64),
            ..file("out/a", b"abc")
        });
        assert_eq!(
            derive(r, 1024, &request(), 0).unwrap_err().code(),
            "backend_error"
        );
    }

    #[test]
    fn deliveries_over_a_bound_are_refused() {
        assert!(derive(raw(&[0; 11], b""), 10, &request(), 0).is_err());
        assert!(derive(raw(b"", &[0; 11]), 10, &request(), 0).is_err());
        let mut many = raw(b"", b"");
        for i in 0..5 {
            many.files.push(file(&format!("out/{i}"), b"x"));
        }
        assert!(derive(many, 1024, &request(), 0).is_err());
        let mut big = raw(b"", b"");
        big.files.push(file("out/big", &[1; 1025]));
        assert!(derive(big, 1024, &request(), 0).is_err());
    }

    #[test]
    fn duplicate_paths_are_refused() {
        let mut r = raw(b"", b"");
        r.files.push(file("out/a", b"1"));
        r.files.push(file("out/a", b"2"));
        assert!(derive(r, 1024, &request(), 0).is_err());
    }

    #[test]
    fn outputs_digest_is_order_independent_and_content_sensitive() {
        let build = |files: Vec<RawFile>| {
            let mut r = raw(b"", b"");
            r.files = files;
            derive(r, 1024, &request(), 0).unwrap().outputs_digest
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
        let c = derive(r, 1024, &request(), 0).unwrap();
        assert!(!format!("{c:?}").contains("s3cr3t"));
        assert!(!format!("{c:#?}").contains("out/a"));
    }

    #[test]
    fn overlapping_secret_values_are_merged_before_replacement() {
        let mut secrets = ResolvedSecrets::new();
        secrets.push(ResolvedSecret::new("abcd"));
        secrets.push(ResolvedSecret::new("cdef"));
        assert_eq!(redact(b"abcdef", &secrets), b"[REDACTED]");
        assert_eq!(redact(b"x abcdef y", &secrets), b"x [REDACTED] y");
    }

    #[test]
    fn encoded_forms_of_a_secret_are_redacted_too() {
        let mut secrets = ResolvedSecrets::new();
        secrets.push(ResolvedSecret::new("p@ss w0rd?"));
        let value = b"p@ss w0rd?";
        for form in [
            hex_encode(value, false),
            hex_encode(value, true),
            base64_encode(value, false, true),
            base64_encode(value, false, false),
            base64_encode(value, true, false),
            percent_encode(value),
            percent_all(value, true),
            percent_all(value, false),
        ] {
            let mut text = b"pre ".to_vec();
            text.extend_from_slice(&form);
            text.extend_from_slice(b" post");
            assert_eq!(redact(&text, &secrets), b"pre [REDACTED] post");
        }
        assert_eq!(base64_encode(b"Man", false, true), b"TWFu");
        assert_eq!(base64_encode(b"Ma", false, true), b"TWE=");
        assert_eq!(base64_encode(b"M", false, false), b"TQ");
        assert_eq!(percent_encode(b"a b/"), b"a%20b%2F");
    }

    #[test]
    fn file_names_are_redacted() {
        let mut r = raw(b"", b"");
        r.secrets.push(ResolvedSecret::new("s3cr3tvalue"));
        r.files.push(file("out/s3cr3tvalue.txt", b"x"));
        let c = derive(r, 1024, &request(), 0).unwrap();
        assert_eq!(c.files[0].path.as_str(), "out/[REDACTED].txt");
    }

    #[test]
    fn colliding_names_after_redaction_are_refused() {
        let mut r = raw(b"", b"");
        r.secrets.push(ResolvedSecret::new("aaa"));
        r.secrets.push(ResolvedSecret::new("bbb"));
        r.files.push(file("out/aaa", b"1"));
        r.files.push(file("out/bbb", b"2"));
        let result = derive(r, 1024, &request(), 0);
        assert!(result.is_err());
    }

    #[test]
    fn fewer_resolved_secrets_than_granted_is_refused() {
        let ok = raw(b"token=hunter2", b"");
        assert!(derive(ok, 1024, &request(), 1).is_err());
        let mut enough = raw(b"token=hunter2", b"");
        enough.secrets.push(ResolvedSecret::new("hunter2"));
        let c = derive(enough, 1024, &request(), 1).unwrap();
        assert_eq!(c.stdout, b"token=[REDACTED]");
    }

    #[test]
    fn delivered_paths_must_match_the_requested_patterns() {
        let mut r = raw(b"", b"");
        r.files.push(file("never-requested/anything", b"y"));
        assert!(derive(r, 1024, &request(), 0).is_err());
        let mut nested = raw(b"", b"");
        nested.files.push(file("out/deep/a", b"y"));
        assert!(
            derive(nested, 1024, &request(), 0).is_err(),
            "`*` stops at `/`"
        );
        let deep = CollectRequest {
            patterns: vec!["out/**".to_owned()],
            ..request()
        };
        let mut nested = raw(b"", b"");
        nested.files.push(file("out/deep/a", b"y"));
        assert!(derive(nested, 1024, &deep, 0).is_ok());
    }

    #[test]
    fn glob_matching_rules() {
        for (pattern, path, expected) in [
            ("out/*", "out/a", true),
            ("out/*", "out/", true),
            ("out/*", "out/a/b", false),
            ("out/**", "out/a/b", true),
            ("**/x.txt", "a/b/x.txt", true),
            ("a?c", "abc", true),
            ("a?c", "a/c", false),
            ("a*c", "abbbc", true),
            ("a*c", "abbbd", false),
            ("exact", "exact", true),
            ("exact", "exact2", false),
            ("*", "", true),
            ("out/?.txt", "out/\u{e9}.txt", true),
            ("out/??.txt", "out/\u{e9}.txt", false),
            ("out/*.txt", "out/\u{e9}\u{e9}.txt", true),
        ] {
            assert_eq!(glob_match(pattern, path), expected, "{pattern} vs {path}");
        }
    }
}
