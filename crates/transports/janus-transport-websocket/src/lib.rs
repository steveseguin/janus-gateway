//! WebSocket transport for Janus Gateway.
//!
//! Accepts WebSocket connections and maps them to Janus signaling sessions.
//! Each WebSocket connection gets a unique client ID.

use dashmap::DashMap;
use janus_core::server::JanusServer;
use janus_transport_api::TransportRequest;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use tokio_tungstenite::tungstenite::Message;

/// WebSocket transport configuration.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct WsTransportConfig {
    /// Address to bind to.
    pub address: String,
    /// Port for the Janus API.
    pub port: u16,
    /// Port for the Admin API (0 = disabled).
    pub admin_port: u16,
    /// WebSocket subprotocol.
    pub subprotocol: String,
}

impl Default for WsTransportConfig {
    fn default() -> Self {
        Self {
            address: "0.0.0.0".into(),
            port: 8188,
            admin_port: 0,
            subprotocol: "janus-protocol".into(),
        }
    }
}

/// A connected WebSocket client.
struct WsClient {
    tx: mpsc::UnboundedSender<Message>,
}

/// The WebSocket transport.
pub struct WsTransport {
    config: WsTransportConfig,
    server: Arc<JanusServer>,
    clients: Arc<DashMap<String, WsClient>>,
}

impl WsTransport {
    pub fn new(config: WsTransportConfig, server: Arc<JanusServer>) -> Self {
        Self {
            config,
            server,
            clients: Arc::new(DashMap::new()),
        }
    }

    /// Start listening for WebSocket connections.
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let addr: SocketAddr = format!("{}:{}", self.config.address, self.config.port).parse()?;
        let listener = TcpListener::bind(addr).await?;
        info!(address = %addr, "WebSocket transport listening");

        let clients = Arc::clone(&self.clients);
        let server = Arc::clone(&self.server);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        let clients = Arc::clone(&clients);
                        let server = Arc::clone(&server);
                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_connection(stream, peer_addr, clients, server).await
                            {
                                error!(peer = %peer_addr, error = %e, "WebSocket error");
                            }
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "failed to accept TCP connection");
                    }
                }
            }
        });

        Ok(())
    }

    /// Send a JSON message to a specific client.
    pub fn send_to_client(
        &self,
        client_id: &str,
        message: serde_json::Value,
    ) -> Result<(), String> {
        if let Some(client) = self.clients.get(client_id) {
            let text = serde_json::to_string(&message).map_err(|e| e.to_string())?;
            client
                .tx
                .send(Message::Text(text.into()))
                .map_err(|e| e.to_string())
        } else {
            Err(format!("client not found: {}", client_id))
        }
    }

    /// Number of connected clients.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    clients: Arc<DashMap<String, WsClient>>,
    server: Arc<JanusServer>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::{SinkExt, StreamExt};

    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let client_id = Uuid::new_v4().to_string();

    info!(client_id = %client_id, peer = %peer_addr, "WebSocket connected");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Message>();

    // Register client
    clients.insert(client_id.clone(), WsClient { tx: msg_tx });

    // Register event sender so server can push events to this client
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<serde_json::Value>();
    server.register_event_sender(&client_id, event_tx);

    // Spawn writer task: forwards messages from the channel to the WebSocket sink
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Read loop: reads from the WebSocket stream and processes messages,
    // also forwards server-pushed events to the client.
    loop {
        tokio::select! {
            msg_result = ws_rx.next() => {
                match msg_result {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(message) => {
                                let request = TransportRequest::new(&client_id);
                                let response = server.process_request(&request, message).await;

                                // Send response back
                                if let Some(client) = clients.get(&client_id) {
                                    let resp_text =
                                        serde_json::to_string(&response).unwrap_or_default();
                                    let _ = client.tx.send(Message::Text(resp_text.into()));
                                }
                            }
                            Err(e) => {
                                warn!(
                                    client_id = %client_id,
                                    error = %e,
                                    "invalid JSON from client"
                                );
                                if let Some(client) = clients.get(&client_id) {
                                    let err = json!({
                                        "janus": "error",
                                        "error": {
                                            "code": 454,
                                            "reason": "Invalid JSON"
                                        }
                                    });
                                    let _ = client
                                        .tx
                                        .send(Message::Text(serde_json::to_string(&err).unwrap().into()));
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        debug!(client_id = %client_id, "WebSocket close frame");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if let Some(client) = clients.get(&client_id) {
                            let _ = client.tx.send(Message::Pong(data));
                        }
                    }
                    Some(Ok(_)) => {} // Ignore binary, pong, etc.
                    Some(Err(e)) => {
                        warn!(client_id = %client_id, error = %e, "WebSocket read error");
                        break;
                    }
                    None => break,
                }
            }
            event = event_rx.recv() => {
                match event {
                    Some(event_json) => {
                        if let Some(client) = clients.get(&client_id) {
                            let text = serde_json::to_string(&event_json).unwrap_or_default();
                            let _ = client.tx.send(Message::Text(text.into()));
                        }
                    }
                    None => {
                        // Event channel closed, server shutting down
                        break;
                    }
                }
            }
        }
    }

    // Cleanup
    server.unregister_event_sender(&client_id);
    clients.remove(&client_id);
    write_handle.abort();
    info!(client_id = %client_id, "WebSocket disconnected");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ws_config() {
        let cfg = WsTransportConfig::default();
        assert_eq!(cfg.port, 8188);
        assert_eq!(cfg.admin_port, 0);
        assert_eq!(cfg.subprotocol, "janus-protocol");
    }

    #[test]
    fn ws_config_deserialize() {
        let toml = r#"
            address = "127.0.0.1"
            port = 9000
            admin_port = 9001
            subprotocol = "janus-protocol"
        "#;
        let cfg: WsTransportConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.address, "127.0.0.1");
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.admin_port, 9001);
    }
}
