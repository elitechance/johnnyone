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

/// Connect to a WebSocket endpoint and return the split read/write streams.
///
/// The URL should be a `wss://` or `ws://` endpoint pointing to the
/// AgentSessionDO WebSocket handler on the Cloudflare Worker.
pub async fn connect(ws_url: &str) -> Result<(WsWrite, WsRead), String> {
    // Parse and validate the URL
    let url = Url::parse(ws_url).map_err(|e| format!("Invalid WebSocket URL: {}", e))?;

    let scheme = url.scheme();
    if scheme != "ws" && scheme != "wss" {
        return Err(format!("Invalid WebSocket scheme: {}. Expected ws or wss.", scheme));
    }

    tracing::debug!(url = %ws_url, "Initiating WebSocket connection");

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
}
