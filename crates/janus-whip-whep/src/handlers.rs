//! Axum handlers for WHIP/WHEP endpoints.
//!
//! Implements the HTTP semantics for RFC 9725 (WHIP) and draft-ietf-wish-whep.

use crate::link_headers;
use crate::sdp_fragment::{self, TRICKLE_ICE_CONTENT_TYPE};
use crate::state::{generate_etag, Resource, ResourceId, ResourceKind, WhipWhepState};
use axum::extract::{Path, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use janus_core::webrtc::{self, PcConfig, PeerConnectionHandle};
use janus_plugin_api::{HandleId, PluginSession, RtpPacket, SessionId};
use std::sync::Arc;
use std::time::Instant;
use str0m::change::SdpOffer;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// CORS helpers
// ---------------------------------------------------------------------------

/// Standard CORS headers for WHIP/WHEP responses.
fn cors_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, PATCH, DELETE, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, If-Match, If-None-Match"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Location, ETag, Link, Accept-Patch"),
    );
    headers
}

/// Add Link headers for ICE servers to a header map.
fn add_link_headers(headers: &mut HeaderMap, state: &WhipWhepState) {
    for link_val in link_headers::link_header_values(&state.ice_servers) {
        if let Ok(val) = HeaderValue::from_str(&link_val) {
            headers.append(header::LINK, val);
        }
    }
}

/// Add the Accept-Patch header.
fn add_accept_patch(headers: &mut HeaderMap) {
    headers.insert(
        "Accept-Patch",
        HeaderValue::from_static(TRICKLE_ICE_CONTENT_TYPE),
    );
}

// ---------------------------------------------------------------------------
// Callbacks adapter for standalone WHIP/WHEP PeerConnections
// ---------------------------------------------------------------------------

/// WebRTC callbacks for WHIP PeerConnections that relay incoming RTP
/// through the fan-out system.
struct WhipWebRtcCallbacks {
    publisher_id: ResourceId,
    state: WhipWhepState,
}

impl janus_core::webrtc::WebRtcCallbacks for WhipWebRtcCallbacks {
    fn on_media_ready(&self, _session: &PluginSession) {
        info!(publisher = %self.publisher_id, "WHIP publisher media ready");
    }

    fn on_media_hangup(&self, _session: &PluginSession, reason: &str) {
        info!(publisher = %self.publisher_id, reason = reason, "WHIP publisher hangup");
    }

    fn on_incoming_rtp(&self, _session: &PluginSession, packet: &RtpPacket) {
        self.state.fanout.relay_rtp(self.publisher_id, packet);
    }

    fn on_incoming_rtcp(&self, _session: &PluginSession, _packet: &janus_plugin_api::RtcpPacket) {
        // RTCP from publisher is not relayed to subscribers
    }
}

/// WebRTC callbacks for WHEP PeerConnections (subscribers).
/// Subscribers are recvonly so we don't expect incoming media.
struct WhepWebRtcCallbacks {
    subscriber_id: ResourceId,
}

impl janus_core::webrtc::WebRtcCallbacks for WhepWebRtcCallbacks {
    fn on_media_ready(&self, _session: &PluginSession) {
        info!(subscriber = %self.subscriber_id, "WHEP subscriber media ready");
    }

    fn on_media_hangup(&self, _session: &PluginSession, reason: &str) {
        info!(subscriber = %self.subscriber_id, reason = reason, "WHEP subscriber hangup");
    }

    fn on_incoming_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {
        // Subscribers shouldn't send RTP
    }

    fn on_incoming_rtcp(&self, _session: &PluginSession, _packet: &janus_plugin_api::RtcpPacket) {}
}

// ---------------------------------------------------------------------------
// Helper: create a PeerConnection for WHIP/WHEP
// ---------------------------------------------------------------------------

async fn create_pc(
    ice_lite: bool,
    callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks>,
) -> Result<PeerConnectionHandle, String> {
    // WHIP/WHEP PeerConnections don't use Janus sessions, so we use
    // dummy IDs. The session/handle IDs are only used for routing in the
    // plugin system, not relevant here.
    let session = PluginSession::new(SessionId(0), HandleId(0));
    let config = PcConfig {
        ice_lite,
        session,
        callbacks,
    };
    webrtc::create_peer_connection(config).await
}

// ---------------------------------------------------------------------------
// POST /whip — WHIP offer (publish)
// ---------------------------------------------------------------------------

pub async fn whip_offer(
    State(state): State<WhipWhepState>,
    req_headers: HeaderMap,
    body: String,
) -> Response {
    // Validate Content-Type
    if !is_sdp_content_type(&req_headers) {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/sdp",
        );
    }

    // Parse SDP offer
    let offer = match SdpOffer::from_sdp_string(&body) {
        Ok(o) => o,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid SDP offer: {e}"),
            );
        }
    };

    // Create PeerConnection with WHIP callbacks
    let resource_id = ResourceId::new();
    let callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks> = Arc::new(WhipWebRtcCallbacks {
        publisher_id: resource_id,
        state: state.clone(),
    });

    let pc_handle = match create_pc(state.ice_lite, callbacks).await {
        Ok(h) => h,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to create PeerConnection: {e}"),
            );
        }
    };

    // Accept the offer
    let answer_sdp = match pc_handle.accept_offer(offer).await {
        Ok(sdp) => sdp,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to generate SDP answer: {e}"),
            );
        }
    };

    // Register the publisher in fan-out
    state.fanout.register_publisher(resource_id);

    // Store the resource
    let etag = generate_etag();
    let resource = Resource {
        id: resource_id,
        kind: ResourceKind::Whip,
        etag: etag.clone(),
        pc_handle,
        created_at: Instant::now(),
        publisher_id: None,
    };
    state.resources.insert(resource_id, resource);

    info!(resource = %resource_id, "WHIP publisher created");

    // Build response
    let mut headers = cors_headers();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/sdp"),
    );
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/resource/{resource_id}")).unwrap(),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{etag}\"")).unwrap(),
    );
    add_link_headers(&mut headers, &state);
    add_accept_patch(&mut headers);

    (StatusCode::CREATED, headers, answer_sdp).into_response()
}

// ---------------------------------------------------------------------------
// POST /whep/:publisher_id — WHEP offer (subscribe)
// ---------------------------------------------------------------------------

pub async fn whep_offer(
    State(state): State<WhipWhepState>,
    Path(publisher_id_str): Path<String>,
    req_headers: HeaderMap,
    body: String,
) -> Response {
    // Validate Content-Type
    if !is_sdp_content_type(&req_headers) {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/sdp",
        );
    }

    // Parse publisher ID
    let publisher_id: ResourceId = match publisher_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Invalid publisher ID");
        }
    };

    // Check publisher exists
    if !state.fanout.has_publisher(&publisher_id) {
        return error_response(StatusCode::NOT_FOUND, "Publisher not found");
    }

    // Parse SDP offer
    let offer = match SdpOffer::from_sdp_string(&body) {
        Ok(o) => o,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid SDP offer: {e}"),
            );
        }
    };

    // Create subscriber PeerConnection
    let resource_id = ResourceId::new();
    let callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks> = Arc::new(WhepWebRtcCallbacks {
        subscriber_id: resource_id,
    });

    let pc_handle = match create_pc(state.ice_lite, callbacks).await {
        Ok(h) => h,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to create PeerConnection: {e}"),
            );
        }
    };

    // Accept the offer
    let answer_sdp = match pc_handle.accept_offer(offer).await {
        Ok(sdp) => sdp,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to generate SDP answer: {e}"),
            );
        }
    };

    // Register as subscriber
    state
        .fanout
        .add_subscriber(publisher_id, resource_id, pc_handle.clone());

    // Store the resource
    let etag = generate_etag();
    let resource = Resource {
        id: resource_id,
        kind: ResourceKind::Whep,
        etag: etag.clone(),
        pc_handle,
        created_at: Instant::now(),
        publisher_id: Some(publisher_id),
    };
    state.resources.insert(resource_id, resource);

    info!(
        resource = %resource_id,
        publisher = %publisher_id,
        "WHEP subscriber created"
    );

    // Build response
    let mut headers = cors_headers();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/sdp"),
    );
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/resource/{resource_id}")).unwrap(),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{etag}\"")).unwrap(),
    );
    add_link_headers(&mut headers, &state);
    add_accept_patch(&mut headers);

    (StatusCode::CREATED, headers, answer_sdp).into_response()
}

// ---------------------------------------------------------------------------
// PATCH /resource/:id — ICE trickle or ICE restart
// ---------------------------------------------------------------------------

pub async fn patch_resource(
    State(state): State<WhipWhepState>,
    Path(id_str): Path<String>,
    req_headers: HeaderMap,
    body: String,
) -> Response {
    let resource_id: ResourceId = match id_str.parse() {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid resource ID"),
    };

    // Check resource exists
    let mut resource_ref = match state.resources.get_mut(&resource_id) {
        Some(r) => r,
        None => return error_response(StatusCode::NOT_FOUND, "Resource not found"),
    };

    // Validate Content-Type
    let content_type = req_headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("application/trickle-ice-sdpfrag") {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/trickle-ice-sdpfrag",
        );
    }

    // Check If-Match header
    let if_match = req_headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if if_match == "*" {
        // ICE restart
        let frag = sdp_fragment::parse_sdp_fragment(&body);

        let remote_ufrag = match frag.ice_ufrag {
            Some(u) => u,
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Missing ice-ufrag in SDP fragment",
                );
            }
        };
        let remote_pwd = match frag.ice_pwd {
            Some(p) => p,
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Missing ice-pwd in SDP fragment",
                );
            }
        };

        // Perform ICE restart
        match resource_ref
            .pc_handle
            .ice_restart(remote_ufrag, remote_pwd)
            .await
        {
            Ok((new_ufrag, new_pwd)) => {
                let new_etag = generate_etag();
                resource_ref.etag = new_etag.clone();

                let response_body = sdp_fragment::generate_ice_restart_fragment(
                    &new_ufrag,
                    &new_pwd,
                    frag.mid.as_deref(),
                );

                let mut headers = cors_headers();
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(TRICKLE_ICE_CONTENT_TYPE),
                );
                headers.insert(
                    header::ETAG,
                    HeaderValue::from_str(&format!("\"{new_etag}\"")).unwrap(),
                );

                (StatusCode::OK, headers, response_body).into_response()
            }
            Err(e) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("ICE restart failed: {e}"),
            ),
        }
    } else {
        // ICE trickle
        // Validate ETag match
        let current_etag = &resource_ref.etag;
        let expected = format!("\"{current_etag}\"");
        if !if_match.is_empty() && if_match != expected && if_match != current_etag.as_str() {
            return error_response(StatusCode::PRECONDITION_FAILED, "ETag mismatch");
        }

        // Parse and apply candidates
        let frag = sdp_fragment::parse_sdp_fragment(&body);
        let mid = frag.mid.unwrap_or_else(|| "0".to_string());

        for candidate in &frag.candidates {
            debug!(resource = %resource_id, candidate = %candidate, "adding ICE candidate");
            let _ = resource_ref
                .pc_handle
                .add_ice_candidate(candidate.clone(), mid.clone())
                .await;
        }

        let headers = cors_headers();
        (StatusCode::NO_CONTENT, headers).into_response()
    }
}

// ---------------------------------------------------------------------------
// DELETE /resource/:id
// ---------------------------------------------------------------------------

pub async fn delete_resource(
    State(state): State<WhipWhepState>,
    Path(id_str): Path<String>,
) -> Response {
    let resource_id: ResourceId = match id_str.parse() {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid resource ID"),
    };

    let resource = match state.resources.remove(&resource_id) {
        Some((_, r)) => r,
        None => return error_response(StatusCode::NOT_FOUND, "Resource not found"),
    };

    // Close the PeerConnection
    resource.pc_handle.close().await;

    match resource.kind {
        ResourceKind::Whip => {
            // Remove publisher from fan-out, get subscriber list
            let subscriber_ids = state.fanout.remove_publisher(resource_id);
            // Close all subscriber PeerConnections
            for sub_id in subscriber_ids {
                if let Some((_, sub_resource)) = state.resources.remove(&sub_id) {
                    sub_resource.pc_handle.close().await;
                    info!(subscriber = %sub_id, "closed WHEP subscriber (publisher removed)");
                }
            }
            info!(resource = %resource_id, "WHIP publisher deleted");
        }
        ResourceKind::Whep => {
            // Unregister from publisher's subscriber list
            if let Some(pub_id) = resource.publisher_id {
                state.fanout.remove_subscriber(pub_id, resource_id);
            }
            info!(resource = %resource_id, "WHEP subscriber deleted");
        }
    }

    let headers = cors_headers();
    (StatusCode::OK, headers).into_response()
}

// ---------------------------------------------------------------------------
// OPTIONS handlers (CORS preflight)
// ---------------------------------------------------------------------------

pub async fn options_whip(State(state): State<WhipWhepState>) -> Response {
    let mut headers = cors_headers();
    add_link_headers(&mut headers, &state);
    add_accept_patch(&mut headers);
    (StatusCode::NO_CONTENT, headers).into_response()
}

pub async fn options_whep(
    State(state): State<WhipWhepState>,
    Path(_publisher_id): Path<String>,
) -> Response {
    let mut headers = cors_headers();
    add_link_headers(&mut headers, &state);
    add_accept_patch(&mut headers);
    (StatusCode::NO_CONTENT, headers).into_response()
}

pub async fn options_resource(
    State(state): State<WhipWhepState>,
    Path(_id): Path<String>,
) -> Response {
    let mut headers = cors_headers();
    add_link_headers(&mut headers, &state);
    add_accept_patch(&mut headers);
    (StatusCode::NO_CONTENT, headers).into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_sdp_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("application/sdp"))
        .unwrap_or(false)
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let mut headers = cors_headers();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain"),
    );
    (status, headers, message.to_string()).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use janus_core::config::NatConfig;
    use tower::ServiceExt;

    fn test_state() -> WhipWhepState {
        WhipWhepState::new(&NatConfig::default())
    }

    fn test_router() -> axum::Router {
        crate::whip_whep_router(test_state())
    }

    #[tokio::test]
    async fn options_whip_returns_cors() {
        let app = test_router();
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/whip")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(resp.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
        assert!(resp.headers().contains_key("accept-patch"));
    }

    #[tokio::test]
    async fn options_resource_returns_cors() {
        let app = test_router();
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/resource/00000000-0000-0000-0000-000000000000")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn whip_rejects_non_sdp_content_type() {
        let app = test_router();
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
    async fn whip_rejects_invalid_sdp() {
        let app = test_router();
        let req = Request::builder()
            .method("POST")
            .uri("/whip")
            .header("content-type", "application/sdp")
            .body(Body::from("not valid sdp"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn whep_404_for_nonexistent_publisher() {
        let app = test_router();
        let req = Request::builder()
            .method("POST")
            .uri("/whep/00000000-0000-0000-0000-000000000000")
            .header("content-type", "application/sdp")
            .body(Body::from("v=0\r\n"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn whep_400_for_invalid_publisher_id() {
        let app = test_router();
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
    async fn delete_404_for_nonexistent_resource() {
        let app = test_router();
        let req = Request::builder()
            .method("DELETE")
            .uri("/resource/00000000-0000-0000-0000-000000000000")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_400_for_invalid_resource_id() {
        let app = test_router();
        let req = Request::builder()
            .method("DELETE")
            .uri("/resource/invalid")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn patch_404_for_nonexistent_resource() {
        let app = test_router();
        let req = Request::builder()
            .method("PATCH")
            .uri("/resource/00000000-0000-0000-0000-000000000000")
            .header("content-type", TRICKLE_ICE_CONTENT_TYPE)
            .body(Body::from("a=ice-ufrag:test\r\na=ice-pwd:test\r\n"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn patch_415_for_wrong_content_type() {
        let state = test_state();
        // Insert a dummy resource
        let id = ResourceId::new();
        let callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks> =
            Arc::new(WhepWebRtcCallbacks {
                subscriber_id: id,
            });
        let pc = create_pc(true, callbacks).await.unwrap();
        state.resources.insert(
            id,
            Resource {
                id,
                kind: ResourceKind::Whip,
                etag: "test".into(),
                pc_handle: pc,
                created_at: Instant::now(),
                publisher_id: None,
            },
        );

        let app = crate::whip_whep_router(state);
        let req = Request::builder()
            .method("PATCH")
            .uri(&format!("/resource/{id}"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn patch_412_for_etag_mismatch() {
        let state = test_state();
        let id = ResourceId::new();
        let callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks> =
            Arc::new(WhepWebRtcCallbacks {
                subscriber_id: id,
            });
        let pc = create_pc(true, callbacks).await.unwrap();
        state.resources.insert(
            id,
            Resource {
                id,
                kind: ResourceKind::Whip,
                etag: "correct-etag".into(),
                pc_handle: pc,
                created_at: Instant::now(),
                publisher_id: None,
            },
        );

        let app = crate::whip_whep_router(state);
        let req = Request::builder()
            .method("PATCH")
            .uri(&format!("/resource/{id}"))
            .header("content-type", TRICKLE_ICE_CONTENT_TYPE)
            .header("if-match", "\"wrong-etag\"")
            .body(Body::from(
                "a=ice-ufrag:test\r\na=ice-pwd:test\r\na=candidate:1 1 UDP 2130706431 10.0.0.1 9999 typ host\r\n",
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn patch_trickle_with_correct_etag() {
        let state = test_state();
        let id = ResourceId::new();
        let callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks> =
            Arc::new(WhepWebRtcCallbacks {
                subscriber_id: id,
            });
        let pc = create_pc(true, callbacks).await.unwrap();
        let etag = "my-etag".to_string();
        state.resources.insert(
            id,
            Resource {
                id,
                kind: ResourceKind::Whip,
                etag: etag.clone(),
                pc_handle: pc,
                created_at: Instant::now(),
                publisher_id: None,
            },
        );

        let app = crate::whip_whep_router(state);
        let req = Request::builder()
            .method("PATCH")
            .uri(&format!("/resource/{id}"))
            .header("content-type", TRICKLE_ICE_CONTENT_TYPE)
            .header("if-match", format!("\"{etag}\""))
            .body(Body::from(
                "a=mid:0\r\na=candidate:1 1 UDP 2130706431 10.0.0.1 9999 typ host\r\n",
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn patch_trickle_without_if_match_succeeds() {
        let state = test_state();
        let id = ResourceId::new();
        let callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks> =
            Arc::new(WhepWebRtcCallbacks {
                subscriber_id: id,
            });
        let pc = create_pc(true, callbacks).await.unwrap();
        state.resources.insert(
            id,
            Resource {
                id,
                kind: ResourceKind::Whip,
                etag: "etag".into(),
                pc_handle: pc,
                created_at: Instant::now(),
                publisher_id: None,
            },
        );

        let app = crate::whip_whep_router(state);
        let req = Request::builder()
            .method("PATCH")
            .uri(&format!("/resource/{id}"))
            .header("content-type", TRICKLE_ICE_CONTENT_TYPE)
            .body(Body::from("a=mid:0\r\n"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn delete_existing_whip_resource() {
        let state = test_state();
        let id = ResourceId::new();
        let callbacks: Arc<dyn janus_core::webrtc::WebRtcCallbacks> =
            Arc::new(WhipWebRtcCallbacks {
                publisher_id: id,
                state: state.clone(),
            });
        let pc = create_pc(true, callbacks).await.unwrap();
        state.fanout.register_publisher(id);
        state.resources.insert(
            id,
            Resource {
                id,
                kind: ResourceKind::Whip,
                etag: "e".into(),
                pc_handle: pc,
                created_at: Instant::now(),
                publisher_id: None,
            },
        );

        let app = crate::whip_whep_router(state.clone());
        let req = Request::builder()
            .method("DELETE")
            .uri(&format!("/resource/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!state.resources.contains_key(&id));
        assert!(!state.fanout.has_publisher(&id));
    }

    #[tokio::test]
    async fn cors_headers_present_on_error_responses() {
        let app = test_router();
        let req = Request::builder()
            .method("DELETE")
            .uri("/resource/00000000-0000-0000-0000-000000000000")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    }

    #[tokio::test]
    async fn options_whep_returns_cors() {
        let app = test_router();
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/whep/00000000-0000-0000-0000-000000000000")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(resp.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    }
}
