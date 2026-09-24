//! A minimal Server-Sent Events decoder shared by the streaming provider ports
//! (ADR 0033, phase 13). It does the SSE **framing only** — accumulate raw bytes,
//! yield the `data:` payload of every event that has become complete — and is
//! provider-agnostic: each provider parses its own JSON payloads on top.
//!
//! The framing rules we honour (a pragmatic subset of the SSE spec, enough for the
//! Anthropic / OpenAI / Gemini streams):
//! - Bytes are buffered and decoded as UTF-8 only once an event is complete, so a
//!   multi-byte character split across two network chunks is never mangled.
//! - CRLF is normalised to LF (also when the CR and LF arrive in different chunks).
//! - Events are separated by a blank line (`\n\n`).
//! - Within an event, every `data:` line contributes; multiple `data:` lines are
//!   joined with `\n` (per the spec). A single leading space after the colon is
//!   stripped. Non-`data:` lines (`event:`, `id:`, `:` comments) are ignored — each
//!   provider's JSON payload carries its own type discriminator.

/// Stateful decoder: feed it byte chunks as they arrive off the wire; it returns the
/// `data:` payloads of each event that completed within that chunk. Bytes that do not
/// yet form a complete event stay buffered for the next [`Self::push`].
#[derive(Default)]
pub struct SseDecoder {
    /// Raw bytes not yet part of a complete event (CRLF already folded to LF, except a
    /// trailing CR whose LF may still arrive in the next chunk).
    buffer: Vec<u8>,
}

impl SseDecoder {
    /// Feed a chunk of raw bytes exactly as read off the wire. Returns the `data:` payload of
    /// every event terminated (by a blank line) within the accumulated buffer so far.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Vec<String> {
        for &byte in chunk {
            // Fold CRLF to LF: a CR immediately followed by LF is dropped, even across chunks.
            if byte == b'\n' && self.buffer.last() == Some(&b'\r') {
                self.buffer.pop();
            }
            self.buffer.push(byte);
        }
        let mut payloads = Vec::new();
        while let Some(boundary) = self.buffer.windows(2).position(|pair| pair == b"\n\n") {
            let event: Vec<u8> = self.buffer.drain(..boundary + 2).collect();
            // The event is complete, so its bytes hold only whole characters.
            if let Some(data) = event_data(&String::from_utf8_lossy(&event)) {
                payloads.push(data);
            }
        }
        payloads
    }

    /// Feed a chunk of already-decoded text (see [`Self::push_bytes`]).
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.push_bytes(chunk.as_bytes())
    }

    /// Flush any trailing event the stream ended without a blank line after (some
    /// servers omit the final `\n\n`). Returns its `data:` payload if present.
    pub fn finish(&mut self) -> Option<String> {
        let event = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
        if event.trim().is_empty() {
            return None;
        }
        event_data(&event)
    }
}

/// Extract and join the `data:` lines of one raw SSE event block. `None` when the
/// block carries no `data:` line (e.g. a bare `event:`/comment heartbeat).
fn event_data(event: &str) -> Option<String> {
    let mut data_lines: Vec<&str> = Vec::new();
    for line in event.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            // A single optional leading space after the colon is part of the framing.
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yields_data_payloads_per_event() {
        let mut decoder = SseDecoder::default();
        let out = decoder.push("event: a\ndata: {\"x\":1}\n\nevent: b\ndata: {\"y\":2}\n\n");
        assert_eq!(out, vec!["{\"x\":1}".to_owned(), "{\"y\":2}".to_owned()]);
    }

    #[test]
    fn buffers_a_partial_event_across_chunks() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push("data: {\"par").is_empty());
        let out = decoder.push("tial\":true}\n\n");
        assert_eq!(out, vec!["{\"partial\":true}".to_owned()]);
    }

    #[test]
    fn normalises_crlf_and_strips_one_leading_space() {
        let mut decoder = SseDecoder::default();
        let out = decoder.push("data: hello\r\n\r\n");
        assert_eq!(out, vec!["hello".to_owned()]);
    }

    #[test]
    fn ignores_comment_and_event_only_blocks() {
        let mut decoder = SseDecoder::default();
        let out = decoder.push(": ping\n\nevent: ping\n\ndata: real\n\n");
        assert_eq!(out, vec!["real".to_owned()]);
    }

    #[test]
    fn a_multibyte_character_split_across_chunks_is_not_corrupted() {
        let wire = "data: {\"text\":\"héllo 👋 世界\"}\n\n".as_bytes();
        // Cut inside "é" (2 bytes), inside "👋" (4 bytes) and inside "世" (3 bytes).
        let cuts = [
            wire.iter().position(|b| *b == 0xC3).unwrap() + 1,
            wire.iter().position(|b| *b == 0xF0).unwrap() + 2,
            wire.iter().position(|b| *b == 0xE4).unwrap() + 1,
        ];
        let mut decoder = SseDecoder::default();
        let mut out = Vec::new();
        let mut start = 0;
        for cut in cuts {
            out.extend(decoder.push_bytes(&wire[start..cut]));
            start = cut;
        }
        out.extend(decoder.push_bytes(&wire[start..]));
        assert_eq!(out, vec!["{\"text\":\"héllo 👋 世界\"}".to_owned()]);
    }

    #[test]
    fn a_crlf_split_across_chunks_still_separates_events() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push_bytes(b"data: one\r\n\r").is_empty());
        let out = decoder.push_bytes(b"\ndata: two\r\n\r\n");
        assert_eq!(out, vec!["one".to_owned(), "two".to_owned()]);
    }

    #[test]
    fn finish_flushes_a_trailing_unterminated_event() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push("data: last").is_empty());
        assert_eq!(decoder.finish(), Some("last".to_owned()));
    }
}
