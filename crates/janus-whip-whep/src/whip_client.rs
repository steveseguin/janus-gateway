//! Outgoing WHIP client for relaying streams to remote WHIP endpoints.
//!
//! Our server acts as a WHIP *client*: it creates a PeerConnection, generates an
//! SDP offer, POSTs it to a remote WHIP endpoint, and applies the returned answer.
//!
//! Enable the `tls` feature for HTTPS support.

use crate::handlers::create_pc;
use crate::state::ResourceId;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use janus_core::webrtc::{PeerConnectionHandle, WebRtcCallbacks};
use janus_plugin_api::{PluginSession, RtcpPacket, RtpPacket};
use std::sync::Arc;
use tracing::info;

/// Configuration for an outgoing WHIP relay.
#[derive(Debug, Clone)]
pub struct WhipOutConfig {
    /// Remote WHIP endpoint URL (e.g., "https://remote-server/whip").
    pub endpoint_url: String,
    /// Optional Bearer token for authentication.
    pub bearer_token: Option<String>,
}

/// Build a hyper HTTPS client using rustls (requires `tls` feature).
#[cfg(feature = "tls")]
fn build_client() -> Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Full<Bytes>,
> {
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    Client::builder(TokioExecutor::new()).build(https)
}

/// Build a hyper HTTP-only client (no TLS — enable the `tls` feature for HTTPS).
#[cfg(not(feature = "tls"))]
fn build_client() -> Client<hyper_util::client::legacy::connect::HttpConnector, Full<Bytes>> {
    Client::builder(TokioExecutor::new()).build_http()
}

/// Validate the URL scheme. Without `tls`, only HTTP is supported.
#[cfg(not(feature = "tls"))]
fn validate_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        return Err(
            "HTTPS URLs require the `tls` feature. Use HTTP or rebuild with --features tls"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(feature = "tls")]
fn validate_url(_url: &str) -> Result<(), String> {
    Ok(())
}

/// Perform outgoing WHIP signaling.
///
/// Creates a local PeerConnection, generates an SDP offer, POSTs it to the
/// remote endpoint, and applies the returned answer.
///
/// Returns `(PeerConnectionHandle, remote_resource_url)` on success.
pub async fn whip_out_publish(
    config: &WhipOutConfig,
    ice_lite: bool,
    relay_id: ResourceId,
) -> Result<(PeerConnectionHandle, Option<String>), String> {
    validate_url(&config.endpoint_url)?;

    // 1. Create PeerConnection with WhipOut callbacks (no-op on incoming RTP)
    let callbacks: Arc<dyn WebRtcCallbacks> = Arc::new(WhipOutWebRtcCallbacks { relay_id });
    let pc_handle = create_pc(ice_lite, callbacks).await?;

    // 2. Create SDP offer with audio + video
    let offer_sdp = pc_handle.create_offer(true, true).await?;

    // 3. POST offer to remote WHIP endpoint
    let uri: hyper::Uri = config
        .endpoint_url
        .parse()
        .map_err(|e| format!("Invalid WHIP endpoint URL: {e}"))?;

    let mut builder = hyper::Request::builder()
        .method(hyper::Method::POST)
        .uri(&uri)
        .header("Content-Type", "application/sdp");

    if let Some(token) = &config.bearer_token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }

    let req = builder
        .body(Full::new(Bytes::from(offer_sdp)))
        .map_err(|e| format!("Failed to build WHIP-out request: {e}"))?;

    let client = build_client();
    let response = client
        .request(req)
        .await
        .map_err(|e| format!("WHIP-out POST failed: {e}"))?;

    if response.status() != hyper::StatusCode::CREATED {
        let status = response.status();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .map(|c| c.to_bytes())
            .unwrap_or_default();
        let body = String::from_utf8_lossy(&body_bytes);
        return Err(format!("Remote WHIP endpoint returned {status}: {body}"));
    }

    // 4. Extract Location header (remote resource URL for DELETE later)
    let remote_resource_url = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 5. Read the SDP answer
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("Failed to read WHIP-out answer body: {e}"))?
        .to_bytes();
    let answer_sdp =
        String::from_utf8(body_bytes.to_vec()).map_err(|e| format!("Invalid UTF-8 in SDP: {e}"))?;

    // 6. Apply the remote answer
    pc_handle.set_remote_answer(answer_sdp).await?;

    info!(
        relay = %relay_id,
        endpoint = %config.endpoint_url,
        remote_resource = ?remote_resource_url,
        "WHIP-out relay negotiated"
    );

    Ok((pc_handle, remote_resource_url))
}

/// Delete a WHIP-out resource on the remote server.
pub async fn whip_out_delete(resource_url: &str, bearer_token: Option<&str>) -> Result<(), String> {
    validate_url(resource_url)?;

    let uri: hyper::Uri = resource_url
        .parse()
        .map_err(|e| format!("Invalid resource URL: {e}"))?;

    let mut builder = hyper::Request::builder()
        .method(hyper::Method::DELETE)
        .uri(&uri);

    if let Some(token) = bearer_token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }

    let req = builder
        .body(Full::new(Bytes::new()))
        .map_err(|e| format!("Failed to build DELETE request: {e}"))?;

    let client = build_client();
    let response = client
        .request(req)
        .await
        .map_err(|e| format!("WHIP-out DELETE failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        info!(url = %resource_url, %status, "remote WHIP-out DELETE returned non-success");
    }

    Ok(())
}

/// WebRTC callbacks for outgoing WHIP-out PeerConnections.
/// These are sendonly — no incoming media is expected.
struct WhipOutWebRtcCallbacks {
    relay_id: ResourceId,
}

impl WebRtcCallbacks for WhipOutWebRtcCallbacks {
    fn on_media_ready(&self, _session: &PluginSession) {
        info!(relay = %self.relay_id, "WHIP-out relay connected");
    }

    fn on_media_hangup(&self, _session: &PluginSession, reason: &str) {
        info!(relay = %self.relay_id, reason, "WHIP-out relay disconnected");
    }

    fn on_incoming_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {
        // Outgoing relay — no incoming media expected
    }

    fn on_incoming_rtcp(&self, _session: &PluginSession, _packet: &RtcpPacket) {}
}
