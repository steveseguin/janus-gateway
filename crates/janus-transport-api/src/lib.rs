//! Transport API for Janus Gateway.
//!
//! Transports are the signaling layer — they carry JSON messages between
//! clients and the Janus core. Each transport (HTTP, WebSocket, MQTT, …)
//! implements the [`JanusTransport`] trait.

pub mod error;

pub use error::{Error, Result};
use janus_plugin_api::{HandleId, SessionId};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// API version that transports must be compatible with.
pub const TRANSPORT_API_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Transport trait
// ---------------------------------------------------------------------------

/// A signaling transport (HTTP, WebSocket, MQTT, …).
#[async_trait]
pub trait JanusTransport: Send + Sync + 'static {
    // --- metadata ---------------------------------------------------------

    fn name(&self) -> &'static str;
    fn package(&self) -> &'static str;
    fn version(&self) -> u32;
    fn version_string(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn author(&self) -> &'static str;

    // --- lifecycle --------------------------------------------------------

    async fn init(
        &mut self,
        callbacks: Arc<dyn TransportCallbacks>,
        config_path: &Path,
    ) -> Result<()>;

    async fn destroy(&mut self) -> Result<()>;

    // --- capabilities -----------------------------------------------------

    /// Whether this transport serves the Janus API.
    fn is_janus_api_enabled(&self) -> bool;

    /// Whether this transport serves the Admin API.
    fn is_admin_api_enabled(&self) -> bool;

    // --- messaging --------------------------------------------------------

    /// Send a message (JSON response/event) back to a client.
    async fn send_message(
        &self,
        request: &TransportRequest,
        message: serde_json::Value,
    ) -> Result<()>;

    /// Notify the transport that a session has ended.
    fn session_over(&self, session_id: SessionId, request: &TransportRequest);
}

// ---------------------------------------------------------------------------
// Callbacks the core provides to transports
// ---------------------------------------------------------------------------

/// Handle the core gives to each transport so it can deliver incoming
/// messages for processing.
#[async_trait]
pub trait TransportCallbacks: Send + Sync + 'static {
    /// Deliver an incoming message from a client to the core.
    async fn incoming_request(
        &self,
        transport_name: &str,
        request: &TransportRequest,
        message: serde_json::Value,
    );

    /// Notify the core that a transport session/connection has gone away.
    fn transport_gone(&self, transport_name: &str, request: &TransportRequest);

    /// Check whether the API secret (if configured) is valid.
    fn is_api_secret_valid(&self, secret: &str) -> bool;

    /// Check whether an auth token is valid.
    fn is_auth_token_valid(&self, token: &str) -> bool;
}

// ---------------------------------------------------------------------------
// Request context
// ---------------------------------------------------------------------------

/// Identifies the originating client connection so the core can route
/// responses back through the correct transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportRequest {
    /// Opaque transport-specific client identifier.
    pub client_id: String,
    /// Whether this came in on the admin API.
    pub admin: bool,
    /// Session ID if already established.
    pub session_id: Option<SessionId>,
    /// Handle ID if already attached.
    pub handle_id: Option<HandleId>,
}

impl TransportRequest {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            admin: false,
            session_id: None,
            handle_id: None,
        }
    }

    pub fn admin(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            admin: true,
            session_id: None,
            handle_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_request_new() {
        let req = TransportRequest::new("ws-client-1");
        assert_eq!(req.client_id, "ws-client-1");
        assert!(!req.admin);
        assert!(req.session_id.is_none());
        assert!(req.handle_id.is_none());
    }

    #[test]
    fn transport_request_admin() {
        let req = TransportRequest::admin("admin-client-1");
        assert_eq!(req.client_id, "admin-client-1");
        assert!(req.admin);
    }

    #[test]
    fn transport_request_serde_roundtrip() {
        let req = TransportRequest {
            client_id: "test".into(),
            admin: false,
            session_id: Some(SessionId(42)),
            handle_id: Some(HandleId(99)),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: TransportRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.client_id, "test");
        assert_eq!(parsed.session_id.unwrap(), SessionId(42));
        assert_eq!(parsed.handle_id.unwrap(), HandleId(99));
    }
}
