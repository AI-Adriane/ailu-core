//! Prompt-compression seam (ADR 0014). The engine ships the trait, a no-op default, and a
//! generic HTTP client; the heavy compressor (LLMLingua-2, which drops low-information
//! tokens) runs as an external service behind `AILU_LLMLINGUA_URL`. [`CompressingGateway`]
//! wraps any [`LlmGateway`] to shrink the **user-message** content (the bulky input/context)
//! before it reaches the provider — mirroring the [`crate::redactor`] seam.
//!
//! Compression is **lossy** (it removes words/punctuation), so it is opt-in: an agent gets it
//! only when its resolved middleware asks for it (`{ kind: "compress" }`); `AILU_LLMLINGUA_URL`
//! only says where the service is. It only touches `user` messages — system instructions,
//! assistant turns and tool results are left intact. On any failure (transport error, timeout,
//! non-2xx, unreadable body) or when the compressed text drops or alters a PII placeholder of the
//! input, the original text is kept (fail-open).

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::LlmError;
use crate::gateway::LlmGateway;
use crate::types::{LlmRequest, LlmResponse};

/// Shrinks bulky prompt text before it reaches a provider.
#[async_trait]
pub trait PromptCompressor: Send + Sync {
    /// Return a possibly-shortened copy of `text`. Default seam impls fail-open (return
    /// the input unchanged on any error).
    async fn compress(&self, text: String) -> String;
}

/// Default: pass everything through unchanged (no compression).
pub struct NoopPromptCompressor;

#[async_trait]
impl PromptCompressor for NoopPromptCompressor {
    async fn compress(&self, text: String) -> String {
        text
    }
}

/// Wraps any [`LlmGateway`] so every `complete()` compresses the **user** message contents
/// first. Compose around the bare gateway: `CompressingGateway::new(inner, compressor)`.
pub struct CompressingGateway {
    inner: Arc<dyn LlmGateway>,
    compressor: Arc<dyn PromptCompressor>,
}

impl CompressingGateway {
    pub fn new(inner: Arc<dyn LlmGateway>, compressor: Arc<dyn PromptCompressor>) -> Self {
        Self { inner, compressor }
    }
}

#[async_trait]
impl LlmGateway for CompressingGateway {
    async fn complete(&self, mut request: LlmRequest) -> Result<LlmResponse, LlmError> {
        for message in request.messages.iter_mut() {
            if message.role == "user" {
                let text = std::mem::take(&mut message.content);
                message.content =
                    compress_preserving_placeholders(self.compressor.as_ref(), text).await;
            }
        }
        self.inner.complete(request).await
    }
}

/// Why a compression kept the original text. Logged by its [`code`](Self::code) only: a log line
/// never carries the text, the service URL or the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressFallback {
    /// The service did not answer within `AILU_LLMLINGUA_TIMEOUT_MS`.
    Timeout,
    /// The request could not be sent or the connection failed.
    Transport,
    /// The service answered with a non-2xx status.
    Status(u16),
    /// The service answered 2xx with a body that is not `{ compressed }`.
    Decode,
    /// The compressed text dropped, altered or added a PII placeholder (`[ENTITY_n]`).
    PlaceholderIntegrity,
}

impl CompressFallback {
    /// A stable, content-free code for logs and metrics.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::Status(_) => "status",
            Self::Decode => "decode",
            Self::PlaceholderIntegrity => "placeholder_integrity",
        }
    }
}

fn log_fallback(reason: CompressFallback) {
    match reason {
        CompressFallback::Status(status) => eprintln!(
            "[llmlingua] compression skipped, original text kept: reason={} status={status}",
            reason.code()
        ),
        _ => eprintln!(
            "[llmlingua] compression skipped, original text kept: reason={}",
            reason.code()
        ),
    }
}

/// A PII placeholder as the control plane and the redaction service write it: `[ENTITY_n]`, an
/// upper-case entity name (`EMAIL`, `EMAIL_ADDRESS`, `PERSON`…) then `_` and a number.
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[A-Z][A-Z0-9_]*_[0-9]+\]").expect("valid placeholder regex"));

fn placeholder_counts(text: &str) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for found in PLACEHOLDER.find_iter(text) {
        *counts.entry(found.as_str()).or_insert(0) += 1;
    }
    counts
}

/// Whether `compressed` carries exactly the PII placeholders of `original`: each `[ENTITY_n]` the
/// same number of times, and none that `original` does not have. A compressor that drops, cuts or
/// rewrites a placeholder (`[EMAIL_1]` → `EMAIL_1`, `[EMAIL_11]`, `[EMAIL]`) fails the check, so the
/// caller keeps the original text: the model never sees a value the vault cannot re-hydrate, and
/// never loses the reference to a masked value. Pure.
#[must_use]
pub fn placeholders_preserved(original: &str, compressed: &str) -> bool {
    placeholder_counts(original) == placeholder_counts(compressed)
}

/// Compress `text` through `compressor`, keeping the original text when the result fails
/// [`placeholders_preserved`] (fail-open on integrity, logged as
/// [`CompressFallback::PlaceholderIntegrity`]). Every compression seam of the engine goes through it.
pub async fn compress_preserving_placeholders(
    compressor: &dyn PromptCompressor,
    text: String,
) -> String {
    let compressed = compressor.compress(text.clone()).await;
    if placeholders_preserved(&text, &compressed) {
        compressed
    } else {
        log_fallback(CompressFallback::PlaceholderIntegrity);
        text
    }
}

/// The JSON body a compression request carries. The answer is `{ compressed }` in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressBody {
    /// `{ "text": <text>, "rate": <rate> }` — Ailu's own compression service (the default).
    #[default]
    Text,
    /// `{ "inputs": <text>, "parameters": { "rate": <rate> } }` — a Hugging Face Inference
    /// Endpoint (inference toolkit), which refuses any body without `inputs` and writes the
    /// refused body, text included, to its logs.
    Inputs,
}

impl CompressBody {
    /// `AILU_LLMLINGUA_BODY`: unset or empty → [`CompressBody::Text`], `text` or `inputs`
    /// (case and surrounding spaces ignored). Any other value is `None`: the caller installs no
    /// compressor rather than guess a shape that the endpoint could refuse and log. Pure.
    #[must_use]
    pub fn from_setting(value: Option<&str>) -> Option<Self> {
        match value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") | Some("text") => Some(Self::Text),
            Some("inputs") => Some(Self::Inputs),
            Some(_) => None,
        }
    }
}

#[derive(Serialize)]
struct TextBody<'a> {
    text: &'a str,
    rate: f64,
}

#[derive(Serialize)]
struct InputsParameters {
    rate: f64,
}

#[derive(Serialize)]
struct InputsBody<'a> {
    inputs: &'a str,
    parameters: InputsParameters,
}

#[derive(Deserialize)]
struct CompressResponse {
    compressed: String,
}

/// The per-request deadline of a compression call when `AILU_LLMLINGUA_TIMEOUT_MS` is unset.
pub const DEFAULT_COMPRESS_TIMEOUT: Duration = Duration::from_millis(2000);

/// `AILU_LLMLINGUA_TIMEOUT_MS` as a duration: a positive integer of milliseconds, else
/// [`DEFAULT_COMPRESS_TIMEOUT`]. Pure.
#[must_use]
pub fn compress_timeout_from(value: Option<&str>) -> Duration {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_COMPRESS_TIMEOUT)
}

/// Calls an external LLMLingua compression service over HTTP. Configure with
/// `AILU_LLMLINGUA_URL` (the service's `POST { text, rate } -> { compressed }` endpoint),
/// `AILU_LLMLINGUA_BODY` (`text`, the default, or `inputs` for a Hugging Face Inference Endpoint:
/// `POST { inputs, parameters: { rate } } -> { compressed }`; see [`CompressBody`]),
/// `AILU_LLMLINGUA_TOKEN` (optional bearer, sent as `Authorization: Bearer …`),
/// `AILU_LLMLINGUA_TIMEOUT_MS` (per-request deadline, default 2000),
/// `AILU_LLMLINGUA_RATE` (target keep-ratio, default 0.5) and `AILU_LLMLINGUA_MIN_CHARS`
/// (skip texts shorter than this, default 240). Returns `None` from `from_env` when the URL
/// is unset, so the caller installs no compression.
///
/// The deadline covers the whole call (connect, headers and body) and is independent of the
/// gateway's read timeout (`AILU_HTTP_READ_TIMEOUT_SECS`, 600 s by default, sized for long
/// completions): a slow compressor costs at most this much per message, then the original text
/// goes on.
pub struct HttpPromptCompressor {
    url: String,
    token: Option<String>,
    rate: f64,
    min_chars: usize,
    timeout: Duration,
    body: CompressBody,
    client: reqwest::Client,
}

impl HttpPromptCompressor {
    /// A compressor for `url`, with an optional bearer `token`, rate 0.5, 240 minimum chars and
    /// the [`DEFAULT_COMPRESS_TIMEOUT`], sending the [`CompressBody::Text`] body.
    pub fn new(url: String, token: Option<String>) -> Self {
        Self {
            url,
            token,
            rate: 0.5,
            min_chars: 240,
            timeout: DEFAULT_COMPRESS_TIMEOUT,
            body: CompressBody::Text,
            client: crate::http::http_client(),
        }
    }

    /// The target keep-ratio sent to the service.
    #[must_use]
    pub fn with_rate(mut self, rate: f64) -> Self {
        self.rate = rate;
        self
    }

    /// Texts shorter than this many characters are not sent.
    #[must_use]
    pub fn with_min_chars(mut self, min_chars: usize) -> Self {
        self.min_chars = min_chars;
        self
    }

    /// The per-request deadline.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The shape of the request body.
    #[must_use]
    pub fn with_body(mut self, body: CompressBody) -> Self {
        self.body = body;
        self
    }

    pub fn from_env() -> Option<Self> {
        let url = std::env::var("AILU_LLMLINGUA_URL")
            .ok()
            .filter(|value| !value.is_empty())?;
        let token = std::env::var("AILU_LLMLINGUA_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        let rate = std::env::var("AILU_LLMLINGUA_RATE")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.5);
        let min_chars = std::env::var("AILU_LLMLINGUA_MIN_CHARS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(240);
        let timeout =
            compress_timeout_from(std::env::var("AILU_LLMLINGUA_TIMEOUT_MS").ok().as_deref());
        let Some(body) =
            CompressBody::from_setting(std::env::var("AILU_LLMLINGUA_BODY").ok().as_deref())
        else {
            eprintln!(
                "[llmlingua] AILU_LLMLINGUA_BODY must be `text` or `inputs`: compression is off"
            );
            return None;
        };
        Some(
            Self::new(url, token)
                .with_rate(rate)
                .with_min_chars(min_chars)
                .with_timeout(timeout)
                .with_body(body),
        )
    }

    async fn request(&self, text: &str) -> Result<String, CompressFallback> {
        let classify = |error: reqwest::Error, otherwise: CompressFallback| {
            if error.is_timeout() {
                CompressFallback::Timeout
            } else {
                otherwise
            }
        };
        let mut builder = self.client.post(&self.url).timeout(self.timeout);
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        builder = match self.body {
            CompressBody::Text => builder.json(&TextBody {
                text,
                rate: self.rate,
            }),
            CompressBody::Inputs => builder.json(&InputsBody {
                inputs: text,
                parameters: InputsParameters { rate: self.rate },
            }),
        };
        let response = builder
            .send()
            .await
            .map_err(|error| classify(error, CompressFallback::Transport))?;
        let status = response.status();
        if !status.is_success() {
            return Err(CompressFallback::Status(status.as_u16()));
        }
        response
            .json::<CompressResponse>()
            .await
            .map(|body| body.compressed)
            .map_err(|error| classify(error, CompressFallback::Decode))
    }
}

#[async_trait]
impl PromptCompressor for HttpPromptCompressor {
    async fn compress(&self, text: String) -> String {
        if text.chars().count() < self.min_chars {
            return text;
        }
        match self.request(&text).await {
            Ok(compressed) => compressed,
            Err(reason) => {
                log_fallback(reason);
                text
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    use super::*;

    #[test]
    fn placeholders_preserved_accepts_the_same_placeholders() {
        assert!(placeholders_preserved(
            "no placeholder here",
            "no placeholder"
        ));
        assert!(placeholders_preserved(
            "Write to [EMAIL_1] about [PERSON_2], then [EMAIL_1] again.",
            "Write [EMAIL_1] [PERSON_2] [EMAIL_1]"
        ));
        assert!(placeholders_preserved(
            "Call [PHONE_NUMBER_12] today please",
            "Call [PHONE_NUMBER_12]"
        ));
        // A redacted secret is not a PII placeholder: compressing around it is not refused.
        assert!(placeholders_preserved(
            "key [REDACTED:OPENAI_KEY] here",
            "key here"
        ));
    }

    #[test]
    fn placeholders_preserved_refuses_a_dropped_or_mangled_placeholder() {
        let input = "Send the contract to [EMAIL_1] and copy [PERSON_2].";
        // Dropped.
        assert!(!placeholders_preserved(
            input,
            "Send contract [EMAIL_1] copy"
        ));
        // Brackets cut.
        assert!(!placeholders_preserved(
            input,
            "Send contract EMAIL_1 copy [PERSON_2]"
        ));
        // Number altered.
        assert!(!placeholders_preserved(
            input,
            "Send contract [EMAIL_11] copy [PERSON_2]"
        ));
        // Number dropped.
        assert!(!placeholders_preserved(
            input,
            "Send contract [EMAIL] copy [PERSON_2]"
        ));
        // Lower-cased.
        assert!(!placeholders_preserved(
            input,
            "Send contract [email_1] copy [PERSON_2]"
        ));
        // One of two occurrences dropped.
        assert!(!placeholders_preserved(
            "[EMAIL_1] wrote to [EMAIL_1]",
            "[EMAIL_1] wrote"
        ));
        // A placeholder the input does not have.
        assert!(!placeholders_preserved("hello there", "hello [PERSON_1]"));
    }

    /// A compressor that answers a fixed text, to drive the integrity check without a network.
    struct Fixed(&'static str);

    #[async_trait]
    impl PromptCompressor for Fixed {
        async fn compress(&self, _text: String) -> String {
            self.0.to_owned()
        }
    }

    #[tokio::test]
    async fn compress_preserving_placeholders_keeps_the_original_on_a_broken_placeholder() {
        let input = "Please write to [EMAIL_1] about the renewal of the contract.".to_owned();
        assert_eq!(
            compress_preserving_placeholders(
                &Fixed("write [EMAIL_1] renewal contract"),
                input.clone()
            )
            .await,
            "write [EMAIL_1] renewal contract"
        );
        assert_eq!(
            compress_preserving_placeholders(&Fixed("write EMAIL renewal contract"), input.clone())
                .await,
            input
        );
    }

    #[test]
    fn compress_timeout_defaults_to_two_seconds() {
        assert_eq!(compress_timeout_from(None), Duration::from_millis(2000));
        assert_eq!(compress_timeout_from(Some("")), DEFAULT_COMPRESS_TIMEOUT);
        assert_eq!(compress_timeout_from(Some("0")), DEFAULT_COMPRESS_TIMEOUT);
        assert_eq!(compress_timeout_from(Some("-5")), DEFAULT_COMPRESS_TIMEOUT);
        assert_eq!(
            compress_timeout_from(Some("soon")),
            DEFAULT_COMPRESS_TIMEOUT
        );
        assert_eq!(
            compress_timeout_from(Some(" 750 ")),
            Duration::from_millis(750)
        );
    }

    /// The whole request: the headers, then `content-length` bytes of body (they may arrive in
    /// separate reads).
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&raw);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if raw.len() >= end + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// A one-shot HTTP server: records the request it receives, waits `delay`, then answers
    /// `body` with `status`.
    fn serve_once(
        status: &'static str,
        body: &'static str,
        delay: Duration,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let request = read_request(&mut stream);
            std::thread::sleep(delay);
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            request
        });
        (format!("http://{address}/compress"), handle)
    }

    const LONG: &str = "This is a long enough user message that it will be sent to the compressor.";

    #[test]
    fn the_body_setting_defaults_to_text_and_refuses_an_unknown_shape() {
        assert_eq!(CompressBody::from_setting(None), Some(CompressBody::Text));
        assert_eq!(
            CompressBody::from_setting(Some("")),
            Some(CompressBody::Text)
        );
        assert_eq!(
            CompressBody::from_setting(Some("text")),
            Some(CompressBody::Text)
        );
        assert_eq!(
            CompressBody::from_setting(Some(" Inputs ")),
            Some(CompressBody::Inputs)
        );
        assert_eq!(CompressBody::from_setting(Some("input")), None);
        assert_eq!(CompressBody::from_setting(Some("json")), None);
    }

    /// The JSON body of a raw HTTP request the one-shot server recorded.
    fn json_body(request: &str) -> serde_json::Value {
        let body = request.split("\r\n\r\n").nth(1).expect("a body");
        serde_json::from_str(body).expect("a JSON body")
    }

    #[tokio::test]
    async fn sends_the_text_body_by_default() {
        let (url, server) = serve_once("200 OK", r#"{"compressed":"short"}"#, Duration::ZERO);
        let compressor = HttpPromptCompressor::new(url, None)
            .with_min_chars(1)
            .with_rate(0.4);
        assert_eq!(compressor.compress(LONG.to_owned()).await, "short");
        let request = server.join().expect("server");
        assert_eq!(
            json_body(&request),
            serde_json::json!({ "text": LONG, "rate": 0.4 })
        );
    }

    #[tokio::test]
    async fn sends_the_inputs_body_to_a_hugging_face_endpoint() {
        let (url, server) = serve_once("200 OK", r#"{"compressed":"short"}"#, Duration::ZERO);
        let compressor = HttpPromptCompressor::new(url, None)
            .with_min_chars(1)
            .with_rate(0.4)
            .with_body(CompressBody::Inputs);
        assert_eq!(compressor.compress(LONG.to_owned()).await, "short");
        let request = server.join().expect("server");
        assert_eq!(
            json_body(&request),
            serde_json::json!({ "inputs": LONG, "parameters": { "rate": 0.4 } })
        );
    }

    #[tokio::test]
    async fn sends_the_bearer_token_when_one_is_set() {
        let (url, server) = serve_once("200 OK", r#"{"compressed":"short"}"#, Duration::ZERO);
        let compressor =
            HttpPromptCompressor::new(url, Some("lingua-test-token".to_owned())).with_min_chars(1);
        assert_eq!(compressor.compress(LONG.to_owned()).await, "short");
        let request = server.join().expect("server").to_ascii_lowercase();
        assert!(
            request.contains("authorization: bearer lingua-test-token"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn sends_no_authorization_without_a_token() {
        let (url, server) = serve_once("200 OK", r#"{"compressed":"short"}"#, Duration::ZERO);
        let compressor = HttpPromptCompressor::new(url, None).with_min_chars(1);
        assert_eq!(compressor.compress(LONG.to_owned()).await, "short");
        let request = server.join().expect("server").to_ascii_lowercase();
        assert!(!request.contains("authorization:"), "{request}");
    }

    #[tokio::test]
    async fn a_slow_service_times_out_and_keeps_the_original_text() {
        let slow = || {
            serve_once(
                "200 OK",
                r#"{"compressed":"too late"}"#,
                Duration::from_millis(1500),
            )
        };
        let compressor = |url: String| {
            HttpPromptCompressor::new(url, None)
                .with_min_chars(1)
                .with_timeout(Duration::from_millis(150))
        };

        let (url, server) = slow();
        let started = Instant::now();
        assert_eq!(compressor(url).compress(LONG.to_owned()).await, LONG);
        assert!(started.elapsed() < Duration::from_millis(1200));
        server.join().expect("server");

        let (url, server) = slow();
        assert_eq!(
            compressor(url).request(LONG).await,
            Err(CompressFallback::Timeout)
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn a_failed_service_keeps_the_original_text() {
        let (url, server) = serve_once("503 Service Unavailable", "{}", Duration::ZERO);
        let compressor = HttpPromptCompressor::new(url, None).with_min_chars(1);
        assert_eq!(
            compressor.request(LONG).await,
            Err(CompressFallback::Status(503))
        );
        server.join().expect("server");

        let (url, server) = serve_once("200 OK", r#"{"unexpected":true}"#, Duration::ZERO);
        let compressor = HttpPromptCompressor::new(url, None).with_min_chars(1);
        assert_eq!(compressor.compress(LONG.to_owned()).await, LONG);
        server.join().expect("server");

        // Nothing listens on port 1: a transport failure, the text goes on unchanged.
        let compressor = HttpPromptCompressor::new("http://127.0.0.1:1/compress".to_owned(), None)
            .with_min_chars(1);
        assert_eq!(compressor.compress(LONG.to_owned()).await, LONG);
    }
}
