//! Integration tests for WHIP/WHEP endpoints.
//!
//! These tests exercise the WHIP/WHEP HTTP endpoints using the axum
//! test utilities (oneshot requests, no real network).

use axum::body::Body;
use axum::http::header;
use axum::http::{Request, StatusCode};
use janus_core::config::{JanusConfig, NatConfig};
use janus_core::server::JanusServer;
use janus_transport_http::janus_api_router;
use janus_whip_whep::sdp_fragment::TRICKLE_ICE_CONTENT_TYPE;
use janus_whip_whep::state::WhipWhepState;
use std::sync::Arc;
use tower::ServiceExt;

/// Build a test app with both Janus API and WHIP/WHEP routes.
fn test_app() -> axum::Router {
    let server = Arc::new(JanusServer::new(JanusConfig::default()));
    let janus_router = janus_api_router(Arc::clone(&server));

    let mut nat = NatConfig::default();
    nat.stun_server = Some("stun.example.com".into());
    nat.stun_port = 3478;
    let whip_state = WhipWhepState::new(&nat);
    let whip_router = janus_whip_whep::whip_whep_router(whip_state);

    janus_router.merge(whip_router)
}

// A minimal valid SDP offer for testing (str0m may or may not accept this)
fn minimal_sdp_offer() -> &'static str {
    "v=0\r\n\
     o=- 0 0 IN IP4 127.0.0.1\r\n\
     s=-\r\n\
     t=0 0\r\n\
     a=group:BUNDLE 0\r\n\
     a=ice-options:trickle\r\n\
     m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
     c=IN IP4 0.0.0.0\r\n\
     a=mid:0\r\n\
     a=sendrecv\r\n\
     a=rtpmap:111 opus/48000/2\r\n\
     a=ice-ufrag:testufrag\r\n\
     a=ice-pwd:testpasswordtestpassword\r\n\
     a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\r\n\
     a=setup:actpass\r\n"
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn options_whip_returns_cors_and_accept_patch() {
    let app = test_app();
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/whip")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(resp
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert!(resp
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_METHODS));
    assert!(resp.headers().contains_key("accept-patch"));
    assert_eq!(
        resp.headers()
            .get("accept-patch")
            .unwrap()
            .to_str()
            .unwrap(),
        TRICKLE_ICE_CONTENT_TYPE
    );
}

#[tokio::test]
async fn options_whip_includes_link_headers_when_stun_configured() {
    let app = test_app();
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/whip")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let link = resp.headers().get(header::LINK);
    assert!(
        link.is_some(),
        "Link header should be present with STUN server"
    );
    let link_str = link.unwrap().to_str().unwrap();
    assert!(link_str.contains("stun:stun.example.com:3478"));
}

#[tokio::test]
async fn whip_rejects_non_sdp() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/whip")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn whip_rejects_garbage_sdp() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/whip")
        .header("content-type", "application/sdp")
        .body(Body::from("this is not valid SDP"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn whep_returns_404_for_nonexistent_publisher() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/whep/00000000-0000-0000-0000-000000000000")
        .header("content-type", "application/sdp")
        .body(Body::from(minimal_sdp_offer()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn whep_returns_400_for_invalid_publisher_id() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/whep/not-a-uuid")
        .header("content-type", "application/sdp")
        .body(Body::from("v=0\r\n"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_returns_404_for_unknown_resource() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/resource/00000000-0000-0000-0000-000000000000")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_returns_400_for_invalid_id() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/resource/garbage")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_returns_404_for_unknown_resource() {
    let app = test_app();
    let req = Request::builder()
        .method("PATCH")
        .uri("/resource/00000000-0000-0000-0000-000000000000")
        .header("content-type", TRICKLE_ICE_CONTENT_TYPE)
        .body(Body::from("a=ice-ufrag:test\r\n"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn janus_api_still_works_alongside_whip_whep() {
    let app = test_app();
    // Verify the Janus API is still accessible
    let req = Request::builder()
        .uri("/janus/info")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["janus"], "server_info");
}

#[tokio::test]
async fn cors_headers_on_error_responses() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/resource/00000000-0000-0000-0000-000000000000")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap()
            .to_str()
            .unwrap(),
        "*"
    );
}

#[tokio::test]
async fn options_whep_returns_cors() {
    let app = test_app();
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/whep/00000000-0000-0000-0000-000000000000")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(resp
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
}

#[tokio::test]
async fn options_resource_returns_cors() {
    let app = test_app();
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/resource/00000000-0000-0000-0000-000000000000")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(resp
        .headers()
        .contains_key(header::ACCESS_CONTROL_ALLOW_HEADERS));
}
