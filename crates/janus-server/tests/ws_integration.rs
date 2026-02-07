//! WebSocket integration tests.
//!
//! These tests start a real WebSocket server and connect to it with a client,
//! exercising the full signaling path.

use futures_util::{SinkExt, StreamExt};
use janus_core::config::JanusConfig;
use janus_core::server::JanusServer;
use janus_plugin_api::JanusPlugin;
use janus_plugin_echotest::EchoTestPlugin;
use janus_transport_websocket::{WsTransport, WsTransportConfig};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Find a free TCP port.
async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

/// Start a WS transport on a random port and return the URL.
async fn start_ws_server() -> (String, Arc<JanusServer>) {
    let port = free_port().await;
    let server = Arc::new(JanusServer::new(JanusConfig::default()));

    // Register echotest plugin
    let mut echotest = EchoTestPlugin::default();
    let callbacks = server.plugin_callbacks();
    echotest.init(callbacks, Path::new("/tmp")).await.unwrap();
    server.register_plugin(Arc::new(echotest));

    let config = WsTransportConfig {
        address: "127.0.0.1".into(),
        port,
        admin_port: 0,
        subprotocol: "janus-protocol".into(),
    };
    let transport = WsTransport::new(config, Arc::clone(&server));
    transport.start().await.unwrap();
    // Give the listener a moment to bind
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (format!("ws://127.0.0.1:{}", port), server)
}

/// Send a JSON message over WS and read the response matching the transaction.
/// Skips over any interleaved event messages (from push_event).
async fn ws_roundtrip(
    tx: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    rx: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    msg: Value,
) -> Value {
    let txn = msg["transaction"].as_str().unwrap_or("").to_string();
    let text = serde_json::to_string(&msg).unwrap();
    tx.send(Message::Text(text.into())).await.unwrap();
    // Read messages until we find one matching our transaction
    loop {
        match rx.next().await.unwrap().unwrap() {
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                // Match on transaction or accept if no transaction expected
                if txn.is_empty() || v["transaction"].as_str() == Some(&txn) {
                    return v;
                }
                // Skip pushed events that don't match our transaction
            }
            other => panic!("expected text message, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn ws_ping_pong() {
    let (url, _server) = start_ws_server().await;
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut tx, mut rx) = ws.split();

    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({"janus": "ping", "transaction": "ws-t1"}),
    )
    .await;
    assert_eq!(resp["janus"], "pong");
    assert_eq!(resp["transaction"], "ws-t1");
}

#[tokio::test]
async fn ws_server_info() {
    let (url, _server) = start_ws_server().await;
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut tx, mut rx) = ws.split();

    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({"janus": "info", "transaction": "ws-info"}),
    )
    .await;
    assert_eq!(resp["janus"], "server_info");
    assert!(resp["name"].is_string());
    assert!(resp["server-name"].is_string());
    assert_eq!(resp["session-timeout"], 60);
    assert!(resp["plugins"].is_object());
}

#[tokio::test]
async fn ws_create_and_destroy_session() {
    let (url, server) = start_ws_server().await;
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut tx, mut rx) = ws.split();

    // Create session
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({"janus": "create", "transaction": "ws-c1"}),
    )
    .await;
    assert_eq!(resp["janus"], "success");
    let session_id = resp["data"]["id"].as_u64().unwrap();
    assert!(session_id > 0);
    assert_eq!(server.sessions().session_count(), 1);

    // Destroy session
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({
            "janus": "destroy",
            "transaction": "ws-d1",
            "session_id": session_id
        }),
    )
    .await;
    assert_eq!(resp["janus"], "success");
    assert_eq!(server.sessions().session_count(), 0);
}

#[tokio::test]
async fn ws_full_session_lifecycle() {
    let (url, _server) = start_ws_server().await;
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut tx, mut rx) = ws.split();

    // Create session
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({"janus": "create", "transaction": "t1"}),
    )
    .await;
    let session_id = resp["data"]["id"].as_u64().unwrap();

    // Attach plugin
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({
            "janus": "attach",
            "transaction": "t2",
            "session_id": session_id,
            "plugin": "janus.plugin.echotest"
        }),
    )
    .await;
    assert_eq!(resp["janus"], "success");
    let handle_id = resp["data"]["id"].as_u64().unwrap();

    // Keepalive
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({
            "janus": "keepalive",
            "transaction": "t3",
            "session_id": session_id
        }),
    )
    .await;
    assert_eq!(resp["janus"], "ack");

    // Send message — synchronous plugin results return inline
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({
            "janus": "message",
            "transaction": "t4",
            "session_id": session_id,
            "handle_id": handle_id,
            "body": {"audio": true, "video": false}
        }),
    )
    .await;
    assert_eq!(resp["janus"], "event");
    assert!(resp["plugindata"]["data"]["echotest"].is_string());

    // Detach
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({
            "janus": "detach",
            "transaction": "t5",
            "session_id": session_id,
            "handle_id": handle_id
        }),
    )
    .await;
    assert_eq!(resp["janus"], "success");

    // Destroy
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({
            "janus": "destroy",
            "transaction": "t6",
            "session_id": session_id
        }),
    )
    .await;
    assert_eq!(resp["janus"], "success");
}

#[tokio::test]
async fn ws_invalid_json_returns_error() {
    let (url, _server) = start_ws_server().await;
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut tx, mut rx) = ws.split();

    // Send invalid JSON
    tx.send(Message::Text("{not valid json".into()))
        .await
        .unwrap();
    let resp = match rx.next().await.unwrap().unwrap() {
        Message::Text(t) => serde_json::from_str::<Value>(&t).unwrap(),
        other => panic!("expected text, got {:?}", other),
    };
    assert_eq!(resp["janus"], "error");
    assert_eq!(resp["error"]["code"], 454);
}

#[tokio::test]
async fn ws_multiple_clients() {
    let (url, server) = start_ws_server().await;

    // Connect 3 clients
    let (ws1, _) = connect_async(&url).await.unwrap();
    let (ws2, _) = connect_async(&url).await.unwrap();
    let (ws3, _) = connect_async(&url).await.unwrap();
    let (mut tx1, mut rx1) = ws1.split();
    let (mut tx2, mut rx2) = ws2.split();
    let (mut tx3, mut rx3) = ws3.split();

    // Each creates a session
    let r1 = ws_roundtrip(
        &mut tx1,
        &mut rx1,
        json!({"janus": "create", "transaction": "c1"}),
    )
    .await;
    let r2 = ws_roundtrip(
        &mut tx2,
        &mut rx2,
        json!({"janus": "create", "transaction": "c2"}),
    )
    .await;
    let r3 = ws_roundtrip(
        &mut tx3,
        &mut rx3,
        json!({"janus": "create", "transaction": "c3"}),
    )
    .await;

    assert_eq!(r1["janus"], "success");
    assert_eq!(r2["janus"], "success");
    assert_eq!(r3["janus"], "success");

    // All different session IDs
    let s1 = r1["data"]["id"].as_u64().unwrap();
    let s2 = r2["data"]["id"].as_u64().unwrap();
    let s3 = r3["data"]["id"].as_u64().unwrap();
    assert_ne!(s1, s2);
    assert_ne!(s2, s3);
    assert_ne!(s1, s3);

    assert_eq!(server.sessions().session_count(), 3);
}

#[tokio::test]
async fn ws_unknown_command() {
    let (url, _server) = start_ws_server().await;
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut tx, mut rx) = ws.split();

    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({"janus": "nonexistent", "transaction": "x1"}),
    )
    .await;
    assert_eq!(resp["janus"], "error");
    assert_eq!(resp["error"]["code"], 455);
}

#[tokio::test]
async fn ws_api_secret_enforced() {
    let mut config = JanusConfig::default();
    config.general.api_secret = Some("ws-secret".into());
    let server = Arc::new(JanusServer::new(config));
    let port = free_port().await;
    let ws_config = WsTransportConfig {
        address: "127.0.0.1".into(),
        port,
        admin_port: 0,
        subprotocol: "janus-protocol".into(),
    };
    let transport = WsTransport::new(ws_config, Arc::clone(&server));
    transport.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (ws, _) = connect_async(format!("ws://127.0.0.1:{}", port))
        .await
        .unwrap();
    let (mut tx, mut rx) = ws.split();

    // Without secret
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({"janus": "ping", "transaction": "t1"}),
    )
    .await;
    assert_eq!(resp["janus"], "error");
    assert_eq!(resp["error"]["code"], 403);

    // With correct secret
    let resp = ws_roundtrip(
        &mut tx,
        &mut rx,
        json!({"janus": "ping", "transaction": "t2", "apisecret": "ws-secret"}),
    )
    .await;
    assert_eq!(resp["janus"], "pong");
}
