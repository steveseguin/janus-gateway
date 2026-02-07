//! Outgoing WHIP client for relaying streams to remote WHIP endpoints.
//!
//! Our server acts as a WHIP *client*: it creates a PeerConnection, generates an
//! SDP offer, POSTs it to a remote WHIP endpoint, and applies the returned answer.

use crate::handlers::create_pc;
use crate::state::ResourceId;
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
    // 1. Create PeerConnection with WhipOut callbacks (no-op on incoming RTP)
    let callbacks: Arc<dyn WebRtcCallbacks> = Arc::new(WhipOutWebRtcCallbacks { relay_id });
    let pc_handle = create_pc(ice_lite, callbacks).await?;

    // 2. Create SDP offer with audio + video
    let offer_sdp = pc_handle.create_offer(true, true).await?;

    // 3. POST offer to remote WHIP endpoint
    let client = reqwest::Client::new();
    let mut request = client
        .post(&config.endpoint_url)
        .header("Content-Type", "application/sdp")
        .body(offer_sdp);
    if let Some(token) = &config.bearer_token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("WHIP-out POST failed: {e}"))?;

    if response.status() != reqwest::StatusCode::CREATED {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Remote WHIP endpoint returned {status}: {body}"));
    }

    // 4. Extract Location header (remote resource URL for DELETE later)
    let remote_resource_url = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 5. Read the SDP answer
    let answer_sdp = response
        .text()
        .await
        .map_err(|e| format!("Failed to read WHIP-out answer body: {e}"))?;

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
    let client = reqwest::Client::new();
    let mut request = client.delete(resource_url);
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
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
