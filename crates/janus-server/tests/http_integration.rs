//! Integration tests for the HTTP transport.
//!
//! These tests start a real HTTP server and make actual HTTP requests
//! to exercise the full request path through the Janus core.

use janus_core::config::JanusConfig;
use janus_core::server::JanusServer;
use janus_plugin_api::JanusPlugin;
use janus_plugin_echotest::EchoTestPlugin;
use janus_transport_http::janus_api_router;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn create_test_app() -> (Router, Arc<JanusServer>) {
    let server = Arc::new(JanusServer::new(JanusConfig::default()));

    // Register echotest plugin
    let mut echotest = EchoTestPlugin::default();
    let callbacks = server.plugin_callbacks();
    echotest.init(callbacks, Path::new("/tmp")).await.unwrap();
    server.register_plugin(Arc::new(echotest));

    let app = janus_api_router(Arc::clone(&server));
    (app, server)
}

async fn post_janus(app: &Router, body: Value) -> Value {
    let req = Request::builder()
        .method("POST")
        .uri("/janus")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn post_session(app: &Router, session_id: u64, body: Value) -> Value {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/janus/{}", session_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn post_handle(app: &Router, session_id: u64, handle_id: u64, body: Value) -> Value {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/janus/{}/{}", session_id, handle_id))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_session_lifecycle() {
    let (app, server) = create_test_app().await;

    // 1. Get server info
    let info = {
        let req = Request::builder()
            .uri("/janus/info")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        v
    };
    assert_eq!(info["janus"], "server_info");
    assert!(info["name"].is_string());

    // 2. Create session
    let resp = post_janus(&app, json!({"janus": "create", "transaction": "t1"})).await;
    assert_eq!(resp["janus"], "success");
    let session_id = resp["data"]["id"].as_u64().unwrap();

    // 3. Attach echotest plugin
    let resp = post_session(
        &app,
        session_id,
        json!({
            "janus": "attach",
            "transaction": "t2",
            "plugin": "janus.plugin.echotest"
        }),
    )
    .await;
    assert_eq!(resp["janus"], "success");
    let handle_id = resp["data"]["id"].as_u64().unwrap();

    // 4. Send keepalive
    let resp = post_session(
        &app,
        session_id,
        json!({"janus": "keepalive", "transaction": "t3"}),
    )
    .await;
    assert_eq!(resp["janus"], "ack");

    // 5. Send message to echotest handle
    let resp = post_handle(
        &app,
        session_id,
        handle_id,
        json!({
            "janus": "message",
            "transaction": "t4",
            "body": {"audio": true, "video": true}
        }),
    )
    .await;
    assert_eq!(resp["janus"], "ack");

    // 6. Detach handle
    let resp = post_session(
        &app,
        session_id,
        json!({
            "janus": "detach",
            "transaction": "t5",
            "handle_id": handle_id
        }),
    )
    .await;
    assert_eq!(resp["janus"], "success");

    // 7. Destroy session
    let resp = post_janus(
        &app,
        json!({
            "janus": "destroy",
            "transaction": "t6",
            "session_id": session_id
        }),
    )
    .await;
    assert_eq!(resp["janus"], "success");

    // Verify session is gone
    assert_eq!(server.sessions().session_count(), 0);
}

#[tokio::test]
async fn multiple_concurrent_sessions() {
    let (app, server) = create_test_app().await;

    // Create 10 sessions
    let mut session_ids = Vec::new();
    for i in 0..10 {
        let resp = post_janus(
            &app,
            json!({"janus": "create", "transaction": format!("create-{i}")}),
        )
        .await;
        assert_eq!(resp["janus"], "success");
        session_ids.push(resp["data"]["id"].as_u64().unwrap());
    }

    assert_eq!(server.sessions().session_count(), 10);

    // Each session attaches a plugin
    for (i, &sid) in session_ids.iter().enumerate() {
        let resp = post_session(
            &app,
            sid,
            json!({
                "janus": "attach",
                "transaction": format!("attach-{i}"),
                "plugin": "janus.plugin.echotest"
            }),
        )
        .await;
        assert_eq!(resp["janus"], "success");
    }

    // Destroy all sessions
    for (i, &sid) in session_ids.iter().enumerate() {
        let resp = post_janus(
            &app,
            json!({
                "janus": "destroy",
                "transaction": format!("destroy-{i}"),
                "session_id": sid
            }),
        )
        .await;
        assert_eq!(resp["janus"], "success");
    }

    assert_eq!(server.sessions().session_count(), 0);
}

#[tokio::test]
async fn error_handling_invalid_session() {
    let (app, _) = create_test_app().await;

    // Keepalive on nonexistent session
    let resp = post_session(
        &app,
        99999,
        json!({"janus": "keepalive", "transaction": "t1"}),
    )
    .await;
    assert_eq!(resp["janus"], "error");

    // Destroy nonexistent session
    let resp = post_janus(
        &app,
        json!({
            "janus": "destroy",
            "transaction": "t2",
            "session_id": 99999
        }),
    )
    .await;
    assert_eq!(resp["janus"], "error");
}

#[tokio::test]
async fn api_secret_authentication() {
    let mut config = JanusConfig::default();
    config.general.api_secret = Some("test-secret-123".into());
    let server = Arc::new(JanusServer::new(config));
    let app = janus_api_router(server);

    // Without secret — denied
    let resp = post_janus(&app, json!({"janus": "ping", "transaction": "t1"})).await;
    assert_eq!(resp["janus"], "error");
    assert_eq!(resp["error"]["code"], 403);

    // With correct secret — works
    let resp = post_janus(
        &app,
        json!({
            "janus": "ping",
            "transaction": "t2",
            "apisecret": "test-secret-123"
        }),
    )
    .await;
    assert_eq!(resp["janus"], "pong");

    // With wrong secret — denied
    let resp = post_janus(
        &app,
        json!({
            "janus": "ping",
            "transaction": "t3",
            "apisecret": "wrong-secret"
        }),
    )
    .await;
    assert_eq!(resp["janus"], "error");
    assert_eq!(resp["error"]["code"], 403);
}

#[tokio::test]
async fn unknown_janus_command() {
    let (app, _) = create_test_app().await;

    let resp = post_janus(
        &app,
        json!({"janus": "bogus_command", "transaction": "t1"}),
    )
    .await;
    assert_eq!(resp["janus"], "error");
    assert_eq!(resp["error"]["code"], 455);
    assert!(resp["error"]["reason"]
        .as_str()
        .unwrap()
        .contains("bogus_command"));
}
