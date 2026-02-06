//! HTTP/REST transport for Janus Gateway.
//!
//! Provides a RESTful API using axum. Supports both the Janus API and
//! the Admin API on configurable ports.

use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use dashmap::DashMap;
use janus_core::server::JanusServer;
use janus_transport_api::TransportRequest;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use uuid::Uuid;

/// HTTP transport configuration.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct HttpTransportConfig {
    pub address: String,
    pub port: u16,
    pub admin_port: u16,
    pub base_path: String,
    pub admin_base_path: String,
    /// Directory to serve static files from (e.g., html/).
    /// If set, files are served at the root path.
    pub static_dir: Option<String>,
    /// Enable WHIP/WHEP endpoints (default: true).
    pub whip_whep: bool,
}

impl Default for HttpTransportConfig {
    fn default() -> Self {
        Self {
            address: "0.0.0.0".into(),
            port: 8088,
            admin_port: 0,
            base_path: "/janus".into(),
            admin_base_path: "/admin".into(),
            static_dir: None,
            whip_whep: true,
        }
    }
}

/// Shared state for axum handlers.
#[derive(Clone)]
struct AppState {
    server: Arc<JanusServer>,
    /// Maps session IDs to their HTTP client IDs for event routing.
    session_clients: Arc<DashMap<u64, String>>,
    /// Event queues per client, for long-poll retrieval.
    event_queues: Arc<DashMap<String, mpsc::UnboundedReceiver<Value>>>,
}

/// Build the Janus API router.
pub fn janus_api_router(server: Arc<JanusServer>) -> Router {
    let state = AppState {
        server: Arc::clone(&server),
        session_clients: Arc::new(DashMap::new()),
        event_queues: Arc::new(DashMap::new()),
    };
    Router::new()
        .route("/janus", post(handle_janus_request))
        .route("/janus/:session_id", post(handle_session_request))
        .route(
            "/janus/:session_id/:handle_id",
            post(handle_handle_request),
        )
        .route("/janus/info", get(handle_info))
        .route("/janus/:session_id/longpoll", get(handle_longpoll))
        .with_state(state)
}

/// Build the Admin API router.
pub fn admin_api_router(server: Arc<JanusServer>) -> Router {
    let state = AppState {
        server,
        session_clients: Arc::new(DashMap::new()),
        event_queues: Arc::new(DashMap::new()),
    };
    Router::new()
        .route("/admin", post(handle_admin_request))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /janus — top-level requests (create, info, ping)
async fn handle_janus_request(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let is_create = body["janus"].as_str() == Some("create");
    let client_id = Uuid::new_v4().to_string();

    // For session creation, register an event sender before processing
    if is_create {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        state.server.register_event_sender(&client_id, event_tx);
        state
            .event_queues
            .insert(client_id.clone(), event_rx);
    }

    let request = TransportRequest::new(&client_id);
    let response = state.server.process_request(&request, body).await;

    // If this was a successful session creation, map the session ID to the client ID
    if is_create && response["janus"] == "success" {
        if let Some(session_id) = response["data"]["id"].as_u64() {
            state.session_clients.insert(session_id, client_id);
        }
    }

    (StatusCode::OK, Json(response))
}

/// POST /janus/:session_id — session-level requests (attach, keepalive, destroy)
async fn handle_session_request(
    State(state): State<AppState>,
    Path(session_id): Path<u64>,
    Json(mut body): Json<Value>,
) -> impl IntoResponse {
    body["session_id"] = json!(session_id);
    // Use the session's registered client_id if available
    let client_id = state
        .session_clients
        .get(&session_id)
        .map(|r| r.value().clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let request = TransportRequest::new(&client_id);
    let response = state.server.process_request(&request, body).await;
    (StatusCode::OK, Json(response))
}

/// POST /janus/:session_id/:handle_id — handle-level requests (message, trickle)
async fn handle_handle_request(
    State(state): State<AppState>,
    Path((session_id, handle_id)): Path<(u64, u64)>,
    Json(mut body): Json<Value>,
) -> impl IntoResponse {
    body["session_id"] = json!(session_id);
    body["handle_id"] = json!(handle_id);
    // Use the session's registered client_id if available
    let client_id = state
        .session_clients
        .get(&session_id)
        .map(|r| r.value().clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let request = TransportRequest::new(&client_id);
    let response = state.server.process_request(&request, body).await;
    (StatusCode::OK, Json(response))
}

/// GET /janus/info — server info
async fn handle_info(State(state): State<AppState>) -> impl IntoResponse {
    let request = TransportRequest::new("http-info");
    let response = state
        .server
        .process_request(
            &request,
            json!({"janus": "info", "transaction": "info"}),
        )
        .await;
    (StatusCode::OK, Json(response))
}

/// GET /janus/:session_id/longpoll — long-poll for events
async fn handle_longpoll(
    State(state): State<AppState>,
    Path(session_id): Path<u64>,
) -> impl IntoResponse {
    let client_id = match state.session_clients.get(&session_id) {
        Some(r) => r.value().clone(),
        None => {
            return (
                StatusCode::OK,
                Json(json!({
                    "janus": "error",
                    "error": {
                        "code": 458,
                        "reason": format!("No such session {session_id}")
                    }
                })),
            );
        }
    };

    // Try to receive an event with a 30-second timeout
    let event = {
        if let Some(mut rx) = state.event_queues.get_mut(&client_id) {
            tokio::select! {
                result = rx.recv() => result,
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => None,
            }
        } else {
            None
        }
    };

    match event {
        Some(event) => {
            debug!(session_id = session_id, "delivering event via long-poll");
            (StatusCode::OK, Json(event))
        }
        None => (
            StatusCode::OK,
            Json(json!({"janus": "keepalive"})),
        ),
    }
}

/// POST /admin — admin API
async fn handle_admin_request(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let client_id = Uuid::new_v4().to_string();
    let request = TransportRequest::admin(&client_id);
    let response = state.server.process_request(&request, body).await;
    (StatusCode::OK, Json(response))
}

/// Start the HTTP transport on the configured address.
pub async fn start_http_transport(
    config: &HttpTransportConfig,
    server: Arc<JanusServer>,
    nat_config: &janus_core::config::NatConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{}:{}", config.address, config.port).parse()?;
    let mut app = janus_api_router(Arc::clone(&server));

    // Merge WHIP/WHEP routes if enabled
    if config.whip_whep {
        let whip_state = janus_whip_whep::state::WhipWhepState::new(nat_config);
        let whip_router = janus_whip_whep::whip_whep_router(whip_state);
        info!("WHIP/WHEP endpoints enabled");
        app = app.merge(whip_router);
    }

    // Serve static files as a fallback if configured
    if let Some(ref dir) = config.static_dir {
        use tower_http::services::ServeDir;
        info!(directory = %dir, "serving static files");
        app = app.fallback_service(ServeDir::new(dir));
    }

    info!(address = %addr, "HTTP transport listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!(error = %e, "HTTP transport error");
        }
    });

    // Admin API on separate port if configured
    if config.admin_port > 0 {
        let admin_addr: SocketAddr =
            format!("{}:{}", config.address, config.admin_port).parse()?;
        let admin_app = admin_api_router(server);

        info!(address = %admin_addr, "HTTP admin transport listening");
        let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
        tokio::spawn(async move {
            if let Err(e) = axum::serve(admin_listener, admin_app).await {
                error!(error = %e, "HTTP admin transport error");
            }
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use janus_core::config::JanusConfig;
    use tower::ServiceExt;

    fn test_app() -> Router {
        let server = Arc::new(JanusServer::new(JanusConfig::default()));
        janus_api_router(server)
    }

    #[tokio::test]
    async fn get_info() {
        let app = test_app();
        let req = Request::builder()
            .uri("/janus/info")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["janus"], "server_info");
    }

    #[tokio::test]
    async fn post_ping() {
        let app = test_app();
        let req = Request::builder()
            .method("POST")
            .uri("/janus")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"janus": "ping", "transaction": "http-test"}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["janus"], "pong");
        assert_eq!(json["transaction"], "http-test");
    }

    #[tokio::test]
    async fn post_create_session() {
        let app = test_app();
        let req = Request::builder()
            .method("POST")
            .uri("/janus")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"janus": "create", "transaction": "c1"}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["janus"], "success");
        assert!(json["data"]["id"].is_number());
    }

    #[test]
    fn default_http_config() {
        let cfg = HttpTransportConfig::default();
        assert_eq!(cfg.port, 8088);
        assert_eq!(cfg.admin_port, 0);
        assert_eq!(cfg.base_path, "/janus");
    }
}
