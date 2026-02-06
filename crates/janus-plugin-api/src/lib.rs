//! Plugin API for Janus Gateway.
//!
//! Every Janus plugin implements the [`JanusPlugin`] trait. The core server
//! loads plugins dynamically and interacts with them exclusively through this
//! interface and the [`PluginCallbacks`] it provides at init time.

pub mod error;
pub mod types;

pub use error::{Error, Result};
pub use types::*;

use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

/// API version that plugins must be compatible with.
pub const PLUGIN_API_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Plugin trait
// ---------------------------------------------------------------------------

/// Every Janus plugin implements this trait.
///
/// The core loads each plugin as a dynamic library (`.so` / `.dylib`) that
/// exports a `janus_plugin_create` symbol returning a boxed `JanusPlugin`.
#[async_trait]
pub trait JanusPlugin: Send + Sync + 'static {
    // --- metadata ---------------------------------------------------------

    /// Human-readable name (e.g. "Janus EchoTest plugin").
    fn name(&self) -> &'static str;

    /// Unique package identifier (e.g. "janus.plugin.echotest").
    fn package(&self) -> &'static str;

    /// Numeric version.
    fn version(&self) -> u32;

    /// Semver string (e.g. "0.1.0").
    fn version_string(&self) -> &'static str;

    /// One-line description.
    fn description(&self) -> &'static str;

    /// Author.
    fn author(&self) -> &'static str;

    // --- lifecycle --------------------------------------------------------

    /// Called once when the plugin is loaded. `callbacks` is the handle back
    /// into the core for relaying media and pushing events.
    async fn init(
        &mut self,
        callbacks: Arc<dyn PluginCallbacks>,
        config_path: &Path,
    ) -> Result<()>;

    /// Called once when the server is shutting down.
    async fn destroy(&mut self) -> Result<()>;

    // --- sessions ---------------------------------------------------------

    /// A new session (browser tab / client) has been created for this plugin.
    async fn create_session(&self, session: &PluginSession) -> Result<()>;

    /// A session is being torn down.
    async fn destroy_session(&self, session: &PluginSession) -> Result<()>;

    /// Return opaque JSON info about a session (for admin/debug).
    fn query_session(&self, session: &PluginSession) -> Result<serde_json::Value>;

    // --- signaling --------------------------------------------------------

    /// Handle an incoming JSON message (with optional JSEP offer/answer).
    async fn handle_message(
        &self,
        session: &PluginSession,
        transaction: &str,
        body: serde_json::Value,
        jsep: Option<Jsep>,
    ) -> Result<PluginResult>;

    /// Handle an admin API message. Default: not implemented.
    async fn handle_admin_message(
        &self,
        _body: serde_json::Value,
    ) -> Result<serde_json::Value> {
        Err(Error::NotImplemented)
    }

    // --- media (default no-op) -------------------------------------------

    /// PeerConnection is ready — media can flow.
    fn setup_media(&self, _session: &PluginSession) {}

    /// PeerConnection has been closed.
    fn hangup_media(&self, _session: &PluginSession, _reason: &str) {}

    /// An RTP packet arrived from the peer.
    fn incoming_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {}

    /// An RTCP packet arrived from the peer.
    fn incoming_rtcp(&self, _session: &PluginSession, _packet: &RtcpPacket) {}

    /// A DataChannel message arrived.
    fn incoming_data(&self, _session: &PluginSession, _label: &str, _data: &[u8]) {}

    /// The DataChannel is ready.
    fn data_ready(&self, _session: &PluginSession) {}

    /// Congestion detected (lost packets).
    fn slow_link(&self, _session: &PluginSession, _uplink: bool, _lost: u32) {}
}

// ---------------------------------------------------------------------------
// Callbacks the core provides to plugins
// ---------------------------------------------------------------------------

/// Handle the core gives to each plugin so it can relay media and events
/// back to the server.
#[async_trait]
pub trait PluginCallbacks: Send + Sync + 'static {
    /// Relay an RTP packet back to the peer.
    fn relay_rtp(&self, session: &PluginSession, packet: &RtpPacket);

    /// Relay an RTCP packet back to the peer.
    fn relay_rtcp(&self, session: &PluginSession, packet: &RtcpPacket);

    /// Relay data over the DataChannel.
    fn relay_data(&self, session: &PluginSession, label: &str, data: &[u8]);

    /// Push an asynchronous event (JSON + optional JSEP) to the client.
    async fn push_event(
        &self,
        session: &PluginSession,
        transaction: &str,
        body: serde_json::Value,
        jsep: Option<Jsep>,
    ) -> Result<()>;

    /// Close the PeerConnection for this session.
    fn close_pc(&self, session: &PluginSession);

    /// End the session entirely.
    fn end_session(&self, session: &PluginSession);

    /// Emit a plugin event for event handlers.
    fn notify_event(&self, plugin_name: &str, event: serde_json::Value);
}
