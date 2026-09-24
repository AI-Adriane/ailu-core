//! Deterministic in-engine secrets redactor (ADR 0032 phase 10).
//!
//! Unlike PII (external Presidio/GLiNER over a URL, fail-open), secrets are a CLOSED, well-known
//! pattern set — so this is a fixed, versioned, constant regex matcher that runs **in-engine,
//! always-on, offline, replay-stable**, with no `eval`/dynamic patterns (the security hard rule).
//! It reuses the existing [`crate::redactor::PiiRedactor`] trait so it drops into the governed
//! `RedactMiddleware`-shaped machinery. Matches are replaced with a **typed one-way placeholder**
//! (the class, never the value or a hash); there is no vault and no hydration.

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;

use crate::error::LlmError;
use crate::redactor::PiiRedactor;
use crate::types::{ContentBlock, LlmRequest};

/// On a detected secret: mask-and-continue (default) or block the call (opt-in, fail-closed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretPolicy {
    Mask,
    Block,
}

struct SecretPattern {
    regex: Regex,
    placeholder: &'static str,
}

/// The versioned, constant secret pattern set. Ordered longest/most-specific first so a
/// broader pattern does not pre-empt a typed class.
static SECRET_PATTERNS: LazyLock<Vec<SecretPattern>> = LazyLock::new(|| {
    let p = |re: &str, placeholder: &'static str| SecretPattern {
        regex: Regex::new(re).expect("valid secret regex"),
        placeholder,
    };
    vec![
        // A whole PEM private-key block (RSA / EC / OPENSSH / DSA / ENCRYPTED / PGP …), body
        // included — masking only the BEGIN line would still send the key material. A block
        // cut off before its END line is masked to the end of the text.
        p(
            r"(?s)-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----|\z)",
            "[REDACTED:PRIVATE_KEY]",
        ),
        p(r"sk-ant-[A-Za-z0-9_-]{20,}", "[REDACTED:ANTHROPIC_KEY]"),
        p(r"sk-or-[A-Za-z0-9_-]{20,}", "[REDACTED:OPENROUTER_KEY]"),
        p(r"sk-proj-[A-Za-z0-9_-]{20,}", "[REDACTED:OPENAI_KEY]"),
        p(r"sk-[A-Za-z0-9]{20,}", "[REDACTED:OPENAI_KEY]"),
        p(r"AKIA[0-9A-Z]{16}", "[REDACTED:AWS_KEY]"),
        p(r"github_pat_[A-Za-z0-9_]{22,}", "[REDACTED:GITHUB_TOKEN]"),
        p(r"gh[oprsu]_[A-Za-z0-9]{36,}", "[REDACTED:GITHUB_TOKEN]"),
        p(r"hf_[A-Za-z0-9]{30,}", "[REDACTED:HUGGINGFACE_TOKEN]"),
        p(r"xox[baprs]-[A-Za-z0-9-]{10,}", "[REDACTED:SLACK_TOKEN]"),
        p(r"AIza[A-Za-z0-9_-]{35}", "[REDACTED:GOOGLE_KEY]"),
        p(r"sk_live_[A-Za-z0-9]{20,}", "[REDACTED:STRIPE_KEY]"),
        p(
            r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            "[REDACTED:JWT]",
        ),
        p(r"(?i)bearer\s+[A-Za-z0-9._-]{20,}", "[REDACTED_SECRET]"),
    ]
});

/// Scrub every known secret pattern from `text`. Returns `(scrubbed, found_any)`.
pub fn scrub_secrets(text: &str) -> (String, bool) {
    let mut out = text.to_owned();
    let mut found = false;
    for pattern in SECRET_PATTERNS.iter() {
        if pattern.regex.is_match(&out) {
            found = true;
            out = pattern
                .regex
                .replace_all(&out, pattern.placeholder)
                .into_owned();
        }
    }
    (out, found)
}

/// Scrub every string inside a JSON value (tool-call arguments). Returns whether anything matched.
fn scrub_json(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => {
            let (out, hit) = scrub_secrets(text);
            if hit {
                *text = out;
            }
            hit
        }
        serde_json::Value::Array(items) => items
            .iter_mut()
            .fold(false, |acc, item| scrub_json(item) | acc),
        serde_json::Value::Object(map) => map
            .values_mut()
            .fold(false, |acc, item| scrub_json(item) | acc),
        _ => false,
    }
}

/// The in-engine secrets floor — a [`PiiRedactor`] that scrubs (or, under `Block`, fails closed).
pub struct RegexSecretsRedactor {
    policy: SecretPolicy,
}

impl RegexSecretsRedactor {
    pub fn new(policy: SecretPolicy) -> Self {
        Self { policy }
    }

    /// Read the policy from `ADRIANE_SECRETS_POLICY` (`block` → fail-closed; anything else → mask).
    pub fn from_env() -> Self {
        let policy = if std::env::var("ADRIANE_SECRETS_POLICY").as_deref() == Ok("block") {
            SecretPolicy::Block
        } else {
            SecretPolicy::Mask
        };
        Self { policy }
    }
}

#[async_trait]
impl PiiRedactor for RegexSecretsRedactor {
    async fn redact_request(&self, mut request: LlmRequest) -> Result<LlmRequest, LlmError> {
        let mut found = false;
        if let Some(system) = request.system.as_mut() {
            let (out, hit) = scrub_secrets(system);
            if hit {
                found = true;
                *system = out;
            }
        }
        for message in request.messages.iter_mut() {
            // `content` is scrubbed even when blocks are present: it stays the text fallback
            // (and the leading text part for some providers), so it is sent too.
            let (out, hit) = scrub_secrets(&message.content);
            if hit {
                found = true;
                message.content = out;
            }
            if let Some(blocks) = message.content_blocks.as_mut() {
                for block in blocks.iter_mut() {
                    if let ContentBlock::Text { text } = block {
                        let (out, hit) = scrub_secrets(text);
                        if hit {
                            found = true;
                            *text = out;
                        }
                    }
                }
            }
            // Replayed assistant tool calls carry their arguments back to the provider.
            if let Some(calls) = message.tool_calls.as_mut() {
                for call in calls.iter_mut() {
                    found |= scrub_json(&mut call.input);
                }
            }
        }
        // Block policy fails closed AFTER scrubbing (the value never leaves the process either way).
        if found && self.policy == SecretPolicy::Block {
            return Err(LlmError::SecretsBlocked(
                "a secret/credential was detected in an outbound request".to_owned(),
            ));
        }
        Ok(request)
    }
    // after_model: strict identity (inherited default) — secrets are one-way, never re-hydrated.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmMessage, LlmProvider};

    fn req(content: &str) -> LlmRequest {
        LlmRequest {
            provider: LlmProvider::Anthropic,
            model: "m".to_owned(),
            messages: vec![LlmMessage::text("user", content)],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            run_id: None,
            response_format: None,
        }
    }

    #[test]
    fn scrubs_known_secret_classes() {
        let (out, found) =
            scrub_secrets("key sk-abcdefghijklmnopqrstuv. and AKIA1234567890ABCDEF end");
        assert!(found);
        assert!(out.contains("[REDACTED:OPENAI_KEY]"));
        assert!(out.contains("[REDACTED:AWS_KEY]"));
        assert!(!out.contains("AKIA1234567890ABCDEF"));
    }

    #[test]
    fn scrubs_provider_keys_of_this_gateway() {
        let secrets = [
            (
                "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789",
                "ANTHROPIC_KEY",
            ),
            (
                "sk-or-v1-0123456789abcdef0123456789abcdef",
                "OPENROUTER_KEY",
            ),
            ("hf_AbCdEfGhIjKlMnOpQrStUvWxYz01234567", "HUGGINGFACE_TOKEN"),
            (
                "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz",
                "GITHUB_TOKEN",
            ),
        ];
        for (secret, class) in secrets {
            let (out, found) = scrub_secrets(&format!("use {secret} please"));
            assert!(found, "{class} not detected");
            assert!(!out.contains(secret), "{class} leaked: {out}");
            assert!(out.contains(&format!("[REDACTED:{class}]")), "{out}");
        }
    }

    #[test]
    fn scrubs_the_whole_private_key_block() {
        let pem = "before\n-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIFHDBOBgkqhkiG9w0BBQ0wQTApBgkq\nAbCdEf==\n-----END ENCRYPTED PRIVATE KEY-----\nafter";
        let (out, found) = scrub_secrets(pem);
        assert!(found);
        assert_eq!(out, "before\n[REDACTED:PRIVATE_KEY]\nafter");

        let truncated = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA";
        let (out, _) = scrub_secrets(truncated);
        assert_eq!(out, "[REDACTED:PRIVATE_KEY]");
    }

    #[tokio::test]
    async fn scrubs_the_text_fallback_and_replayed_tool_arguments() {
        let secret = "sk-abcdefghijklmnopqrstuvwxyz0";
        let mut message = LlmMessage::text("assistant", format!("see {secret}"));
        message.content_blocks = Some(vec![ContentBlock::Text {
            text: format!("see {secret}"),
        }]);
        message.tool_calls = Some(vec![crate::types::LlmToolCall {
            id: "call-1".to_owned(),
            name: "http".to_owned(),
            input: serde_json::json!({ "headers": { "authorization": secret }, "n": 1 }),
        }]);
        let mut request = req("hi");
        request.messages.push(message);

        let out = RegexSecretsRedactor::new(SecretPolicy::Mask)
            .redact_request(request)
            .await
            .unwrap();
        let rendered = serde_json::to_string(&out).unwrap();
        assert!(!rendered.contains(secret), "{rendered}");
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let (out, found) = scrub_secrets("just a normal prompt about cats");
        assert!(!found);
        assert_eq!(out, "just a normal prompt about cats");
    }

    #[tokio::test]
    async fn mask_policy_scrubs_and_continues() {
        let r = RegexSecretsRedactor::new(SecretPolicy::Mask);
        let out = r
            .redact_request(req("here is ghp_012345678901234567890123456789012345 ok"))
            .await
            .unwrap();
        assert!(out.messages[0].content.contains("[REDACTED:GITHUB_TOKEN]"));
        assert!(!out.messages[0].content.contains("ghp_0123456789"));
    }

    #[tokio::test]
    async fn block_policy_fails_closed_on_a_secret() {
        let r = RegexSecretsRedactor::new(SecretPolicy::Block);
        let err = r
            .redact_request(req("token sk-abcdefghijklmnopqrstuvwxyz0"))
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::SecretsBlocked(_)));
    }

    #[tokio::test]
    async fn block_policy_passes_clean_requests() {
        let r = RegexSecretsRedactor::new(SecretPolicy::Block);
        assert!(r.redact_request(req("hello world")).await.is_ok());
    }
}
