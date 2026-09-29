//! Media resolution seam (ADR 0030 phase 9c).
//!
//! Multimodal content blocks may carry their bytes as an `Artifact` reference (the
//! ref-not-bytes default — small + stable in checkpoints) rather than inline base64. A
//! [`MediaResolver`] resolves those references to inline bytes (or a URL) **at the gateway
//! boundary, just before the adapter runs** — so the adapters stay pure (they only ever
//! format `Base64`/`Url`) and `GraphState`/checkpoints/events never carry the bytes.

use async_trait::async_trait;

use crate::error::LlmError;
use crate::types::{ContentBlock, LlmRequest, MediaSource};

/// Hard cap on inline base64 media length (ADR 0030 D5): never let unbounded inline media
/// reach a provider request or a checkpoint. ~6 MiB of base64 ≈ ~4.5 MiB of raw bytes.
pub const MAX_INLINE_MEDIA_BASE64_LEN: usize = 6 * 1024 * 1024;

#[async_trait]
pub trait MediaResolver: Send + Sync {
    /// Resolve a media source into a form the adapters can serialize directly. `Base64` and
    /// `Url` pass through unchanged; an `Artifact` reference is resolved to its bytes. Returns
    /// an error when a reference cannot be resolved.
    async fn resolve(&self, source: &MediaSource) -> Result<MediaSource, LlmError>;
}

/// Resolve every media block's source in `request` (in place) via `resolver`, enforcing the
/// inline-size cap. Text and tool blocks are untouched; a request with no content blocks is a
/// no-op.
pub async fn resolve_request_media(
    request: &mut LlmRequest,
    resolver: &dyn MediaResolver,
) -> Result<(), LlmError> {
    for message in request.messages.iter_mut() {
        let Some(blocks) = message.content_blocks.as_mut() else {
            continue;
        };
        for block in blocks.iter_mut() {
            let source = match block {
                ContentBlock::Image { source }
                | ContentBlock::Audio { source }
                | ContentBlock::File { source } => source,
                ContentBlock::Text { .. } => continue,
            };
            let resolved = resolver.resolve(source).await?;
            if let MediaSource::Base64 { data, .. } = &resolved {
                if data.len() > MAX_INLINE_MEDIA_BASE64_LEN {
                    return Err(LlmError::MediaResolution(format!(
                        "inline media exceeds the {MAX_INLINE_MEDIA_BASE64_LEN}-byte cap; \
                         reference it from the artifact store instead"
                    )));
                }
            }
            *source = resolved;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LlmMessage;

    /// A resolver that turns any Artifact into fixed base64 bytes; passes Base64/Url through.
    struct FakeResolver;

    #[async_trait]
    impl MediaResolver for FakeResolver {
        async fn resolve(&self, source: &MediaSource) -> Result<MediaSource, LlmError> {
            match source {
                MediaSource::Artifact { media_type, .. } => Ok(MediaSource::Base64 {
                    media_type: media_type.clone(),
                    data: "RESOLVED".to_owned(),
                }),
                other => Ok(other.clone()),
            }
        }
    }

    fn request_with(source: MediaSource) -> LlmRequest {
        LlmRequest {
            web_search: None,
            provider: crate::types::LlmProvider::Anthropic,
            model: "m".to_owned(),
            messages: vec![LlmMessage::with_blocks(
                "user",
                vec![ContentBlock::Image { source }],
            )],
            system: None,
            tools: None,
            max_tokens: None,
            temperature: None,
            response_format: None,
            run_id: None,
        }
    }

    #[tokio::test]
    async fn resolves_artifact_sources_to_inline_bytes() {
        let mut req = request_with(MediaSource::Artifact {
            artifact_id: "a1".to_owned(),
            version: None,
            media_type: "image/png".to_owned(),
        });
        resolve_request_media(&mut req, &FakeResolver)
            .await
            .unwrap();
        let blocks = req.messages[0].content_blocks.as_ref().unwrap();
        match &blocks[0] {
            ContentBlock::Image {
                source: MediaSource::Base64 { media_type, data },
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "RESOLVED");
            }
            other => panic!("expected resolved base64, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn passes_base64_through_unchanged() {
        let mut req = request_with(MediaSource::Base64 {
            media_type: "image/png".to_owned(),
            data: "AAAA".to_owned(),
        });
        resolve_request_media(&mut req, &FakeResolver)
            .await
            .unwrap();
        let blocks = req.messages[0].content_blocks.as_ref().unwrap();
        assert!(matches!(
            &blocks[0],
            ContentBlock::Image {
                source: MediaSource::Base64 { data, .. }
            } if data == "AAAA"
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_inline_media() {
        let mut req = request_with(MediaSource::Base64 {
            media_type: "image/png".to_owned(),
            data: "A".repeat(MAX_INLINE_MEDIA_BASE64_LEN + 1),
        });
        let err = resolve_request_media(&mut req, &FakeResolver)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::MediaResolution(_)));
    }
}
