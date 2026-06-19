use futures_util::stream::{SplitSink, SplitStream};
use futures_util::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use url::Url;

/// Type alias for the write half of the WebSocket stream.
pub type WsWrite = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

/// Type alias for the read half of the WebSocket stream.
pub type WsRead = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

/// Redact any &token= value from a URL before logging (never emit credential material).
/// The token is always appended last in the callers (see mod.rs), so split_once is sufficient.
/// Strips only the value; keeps the key for debuggability.
pub fn redact_token_in_url(url: &str) -> String {
    match url.split_once("&token=") {
        Some((base, _)) => format!("{}&token=<redacted>", base),
        None => url.to_string(),
    }
}

/// Connect to a WebSocket endpoint and return the split read/write streams.
///
/// The URL should be a `wss://` or `ws://` endpoint pointing to the
/// AgentSessionDO WebSocket handler on the Cloudflare Worker.
pub async fn connect(ws_url: &str) -> Result<(WsWrite, WsRead), String> {
    // Parse and validate the URL
    let url = Url::parse(ws_url).map_err(|e| format!("Invalid WebSocket URL: {}", e))?;

    let scheme = url.scheme();
    if scheme != "ws" && scheme != "wss" {
        return Err(format!(
            "Invalid WebSocket scheme: {}. Expected ws or wss.",
            scheme
        ));
    }

    let log_url = redact_token_in_url(ws_url);
    tracing::debug!(url = %log_url, "Initiating WebSocket connection");

    // Perform the WebSocket handshake
    let (ws_stream, response) = connect_async(ws_url)
        .await
        .map_err(|e| format!("WebSocket connection failed: {}", e))?;

    tracing::info!(
        status = %response.status(),
        "WebSocket handshake completed"
    );

    // Split the stream into read and write halves for concurrent use
    let (write, read) = ws_stream.split();

    Ok((write, read))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_validation() {
        // Valid URLs should parse successfully
        assert!(Url::parse("wss://example.com/ws").is_ok());
        assert!(Url::parse("ws://localhost:8080/ws").is_ok());

        // Invalid URLs should fail
        assert!(Url::parse("not-a-url").is_err());
    }

    #[test]
    fn test_log_redaction_no_token_leak() {
        // The shared helper (used by production connect() log) must redact credential material.
        let full_with_jk = "wss://example.com/api/relay/ws?nodeId=foo&clientType=desktop&token=jk_abc123secret";
        let redacted = redact_token_in_url(full_with_jk);
        assert!(!redacted.contains("jk_"), "log must not contain jk_ prefix: {}", redacted);
        assert!(!redacted.contains("abc123secret"), "log must not contain token value: {}", redacted);
        assert!(redacted.contains("&token=<redacted>"));

        let full_with_jwt = "wss://example.com/api/relay/ws?nodeId=bar&clientType=desktop&token=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dummysig";
        let redacted2 = redact_token_in_url(full_with_jwt);
        assert!(!redacted2.contains("eyJ"), "log must not contain JWT start: {}", redacted2);
    }
}
