//! The HTTP client configuration shared by every outbound call the gateway makes (provider
//! adapters, the PII redactor, the prompt compressor, the cross-encoder).

use std::time::Duration;

/// How long to wait for a TCP/TLS connection to be established.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Default longest silence tolerated while waiting for a response: for the headers of a
/// non-streamed completion (sent only once generation ends), or between two chunks of a streamed
/// one. Generous so long reasoning completions finish, but finite so a peer that accepts the
/// connection and never answers cannot hang a run forever. Override with
/// `AILU_HTTP_READ_TIMEOUT_SECS`.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(600);

fn read_timeout() -> Duration {
    std::env::var("AILU_HTTP_READ_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_READ_TIMEOUT)
}

/// A client with connect/read timeouts that never follows redirects.
///
/// Redirects are refused because provider keys travel in headers reqwest keeps when it follows a
/// redirect to another host (it strips only `Authorization` and cookies): Anthropic's `x-api-key`
/// and Gemini's `x-goog-api-key` would be re-sent to wherever a 3xx points. The APIs called here
/// answer directly; a redirect surfaces as the non-2xx error it is.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(read_timeout())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static HTTP client configuration is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn does_not_follow_a_redirect_carrying_a_key_elsewhere() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0_u8; 2048];
            let _ = stream.read(&mut buffer);
            // Point the redirect at a port nothing listens on: following it would fail to connect.
            let _ = write!(
                stream,
                "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://127.0.0.1:1/steal\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
        });

        let response = http_client()
            .post(format!("http://{address}/v1/messages"))
            .header("x-api-key", "provider-secret")
            .send()
            .await
            .expect("the 307 itself is returned, not followed");
        assert_eq!(response.status().as_u16(), 307);
        server.join().expect("server");
    }
}
