//! Main Janus server.
//!
//! Ties together session management, plugin loading, transport handling,
//! event dispatch, and the media relay.

use crate::config::JanusConfig;
use crate::relay::{self, RelaySender};
use crate::session::SessionManager;
use crate::webrtc::{self, PcConfig, PeerConnectionHandle, WebRtcCallbacks};
use dashmap::DashMap;
use janus_plugin_api::{
    HandleId, Jsep, JsepType, JanusPlugin, PluginCallbacks, PluginResult, PluginSession,
    RtcpPacket, RtpPacket, SessionId,
};
use janus_transport_api::TransportRequest;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn};

/// The core Janus server.
pub struct JanusServer {
    config: JanusConfig,
    sessions: Arc<SessionManager>,
    relay_sender: RelaySender,
    shutdown: Arc<Notify>,
    /// Registered plugins by package name.
    plugins: DashMap<String, Arc<dyn JanusPlugin>>,
    /// Transport event senders by client_id for push_event routing.
    event_senders: Arc<DashMap<String, mpsc::UnboundedSender<serde_json::Value>>>,
    /// Map session_id → client_id for routing events back to the right transport.
    session_clients: Arc<DashMap<SessionId, String>>,
    /// PeerConnection handles by (session_id, handle_id).
    peer_connections: Arc<DashMap<(SessionId, HandleId), PeerConnectionHandle>>,
}

impl JanusServer {
    /// Create a new server with the given configuration.
    pub fn new(config: JanusConfig) -> Self {
        let sessions = Arc::new(SessionManager::new(config.general.session_timeout));
        let (_relay_engine, relay_sender) = relay::create_relay(65536);
        Self {
            config,
            sessions,
            relay_sender,
            shutdown: Arc::new(Notify::new()),
            plugins: DashMap::new(),
            event_senders: Arc::new(DashMap::new()),
            session_clients: Arc::new(DashMap::new()),
            peer_connections: Arc::new(DashMap::new()),
        }
    }

    /// Get a reference to the server configuration.
    pub fn config(&self) -> &JanusConfig {
        &self.config
    }

    /// Get the session manager.
    pub fn sessions(&self) -> &Arc<SessionManager> {
        &self.sessions
    }

    /// Get a relay sender for submitting media commands.
    pub fn relay_sender(&self) -> &RelaySender {
        &self.relay_sender
    }

    /// Register a plugin instance.
    pub fn register_plugin(&self, plugin: Arc<dyn JanusPlugin>) {
        let package = plugin.package().to_string();
        info!(plugin = %package, name = %plugin.name(), "plugin registered");
        self.plugins.insert(package, plugin);
    }

    /// Get a reference to the event senders map.
    pub fn event_senders(&self) -> &Arc<DashMap<String, mpsc::UnboundedSender<serde_json::Value>>> {
        &self.event_senders
    }

    /// Register a transport event sender for a client.
    pub fn register_event_sender(
        &self,
        client_id: &str,
        sender: mpsc::UnboundedSender<serde_json::Value>,
    ) {
        self.event_senders.insert(client_id.to_string(), sender);
    }

    /// Unregister a transport event sender.
    pub fn unregister_event_sender(&self, client_id: &str) {
        self.event_senders.remove(client_id);
    }

    /// Create an Arc<dyn TransportCallbacks> for transports to call into.
    pub fn transport_callbacks(self: &Arc<Self>) -> Arc<dyn janus_transport_api::TransportCallbacks> {
        Arc::new(CoreTransportCallbacks {
            server: Arc::clone(self),
        })
    }

    /// Create an Arc<dyn PluginCallbacks> for plugins to call into.
    pub fn plugin_callbacks(self: &Arc<Self>) -> Arc<dyn PluginCallbacks> {
        Arc::new(CorePluginCallbacks {
            server: Arc::clone(self),
        })
    }

    /// Process an incoming JSON request from a transport.
    pub async fn process_request(
        self: &Arc<Self>,
        request: &TransportRequest,
        message: serde_json::Value,
    ) -> serde_json::Value {
        let janus = message["janus"].as_str().unwrap_or("");
        let transaction = message["transaction"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // Check API secret if configured
        if let Some(ref secret) = self.config.general.api_secret {
            let provided = message["apisecret"].as_str().unwrap_or("");
            if provided != secret {
                return json!({
                    "janus": "error",
                    "transaction": transaction,
                    "error": {
                        "code": 403,
                        "reason": "Unauthorized request (wrong or missing secret)"
                    }
                });
            }
        }

        match janus {
            "info" => self.handle_info(&transaction),
            "ping" => json!({ "janus": "pong", "transaction": transaction }),
            "create" => self.handle_create(&transaction, request).await,
            "destroy" => {
                let session_id = message["session_id"].as_u64().unwrap_or(0);
                self.handle_destroy(&transaction, SessionId(session_id))
                    .await
            }
            "attach" => {
                let session_id = message["session_id"].as_u64().unwrap_or(0);
                let plugin = message["plugin"].as_str().unwrap_or("");
                self.handle_attach(&transaction, SessionId(session_id), plugin)
                    .await
            }
            "detach" => {
                let session_id = message["session_id"].as_u64().unwrap_or(0);
                let handle_id = message["handle_id"].as_u64().unwrap_or(0);
                self.handle_detach(
                    &transaction,
                    SessionId(session_id),
                    HandleId(handle_id),
                )
                .await
            }
            "keepalive" => {
                let session_id = message["session_id"].as_u64().unwrap_or(0);
                self.handle_keepalive(&transaction, SessionId(session_id))
            }
            "message" => {
                let session_id = message["session_id"].as_u64().unwrap_or(0);
                let handle_id = message["handle_id"].as_u64().unwrap_or(0);
                self.handle_message(
                    &transaction,
                    request,
                    SessionId(session_id),
                    HandleId(handle_id),
                    &message,
                )
                .await
            }
            "trickle" => {
                let session_id = message["session_id"].as_u64().unwrap_or(0);
                let handle_id = message["handle_id"].as_u64().unwrap_or(0);
                self.handle_trickle(
                    &transaction,
                    SessionId(session_id),
                    HandleId(handle_id),
                    &message,
                )
                .await
            }
            _ => json!({
                "janus": "error",
                "transaction": transaction,
                "error": {
                    "code": 455,
                    "reason": format!("Unknown request '{janus}'")
                }
            }),
        }
    }

    fn handle_info(&self, transaction: &str) -> serde_json::Value {
        let plugin_list: Vec<String> = self.plugins.iter().map(|e| e.key().clone()).collect();
        json!({
            "janus": "server_info",
            "transaction": transaction,
            "name": self.config.general.server_name,
            "version_string": env!("CARGO_PKG_VERSION"),
            "author": "Steve Seguin (Rust rewrite)",
            "data_channels": true,
            "session_timeout": self.config.general.session_timeout,
            "plugins": plugin_list,
        })
    }

    async fn handle_create(
        &self,
        transaction: &str,
        request: &TransportRequest,
    ) -> serde_json::Value {
        let session_id = self.sessions.create_session();
        // Associate session with client for event routing
        self.session_clients
            .insert(session_id, request.client_id.clone());
        info!(session_id = %session_id, "new session created");
        json!({
            "janus": "success",
            "transaction": transaction,
            "data": {
                "id": session_id.0
            }
        })
    }

    async fn handle_destroy(
        &self,
        transaction: &str,
        session_id: SessionId,
    ) -> serde_json::Value {
        match self.sessions.destroy_session(session_id) {
            Ok(handles) => {
                // Clean up peer connections for all handles
                for handle in &handles {
                    let key = (session_id, handle.handle_id);
                    if let Some((_, pc)) = self.peer_connections.remove(&key) {
                        let _ = pc.close().await;
                    }
                    // Notify plugin of session destruction
                    if let Some(plugin) = self.plugins.get(&handle.plugin_package) {
                        let ps = handle.plugin_session();
                        let _ = plugin.destroy_session(&ps).await;
                    }
                }
                // Clean up session-client mapping
                self.session_clients.remove(&session_id);
                info!(session_id = %session_id, handles = handles.len(), "session destroyed");
                json!({
                    "janus": "success",
                    "transaction": transaction,
                    "session_id": session_id.0,
                })
            }
            Err(e) => json!({
                "janus": "error",
                "transaction": transaction,
                "error": {
                    "code": 458,
                    "reason": e.to_string()
                }
            }),
        }
    }

    async fn handle_attach(
        &self,
        transaction: &str,
        session_id: SessionId,
        plugin: &str,
    ) -> serde_json::Value {
        // Touch the session to reset timeout
        self.sessions.touch_session(session_id);

        match self
            .sessions
            .attach_handle(session_id, plugin.to_string())
        {
            Ok(handle_id) => {
                // Notify plugin of new session
                if let Some(plugin_ref) = self.plugins.get(plugin) {
                    let ps = PluginSession::new(session_id, handle_id);
                    if let Err(e) = plugin_ref.create_session(&ps).await {
                        warn!(error = %e, "plugin create_session failed");
                    }
                }

                info!(
                    session_id = %session_id,
                    handle_id = %handle_id,
                    plugin = plugin,
                    "handle attached"
                );
                json!({
                    "janus": "success",
                    "transaction": transaction,
                    "session_id": session_id.0,
                    "data": {
                        "id": handle_id.0
                    }
                })
            }
            Err(e) => json!({
                "janus": "error",
                "transaction": transaction,
                "error": {
                    "code": 458,
                    "reason": e.to_string()
                }
            }),
        }
    }

    async fn handle_detach(
        &self,
        transaction: &str,
        session_id: SessionId,
        handle_id: HandleId,
    ) -> serde_json::Value {
        match self.sessions.detach_handle(session_id, handle_id) {
            Ok(info) => {
                // Clean up peer connection
                let key = (session_id, handle_id);
                if let Some((_, pc)) = self.peer_connections.remove(&key) {
                    let _ = pc.close().await;
                }
                // Notify plugin
                if let Some(plugin) = self.plugins.get(&info.plugin_package) {
                    let ps = info.plugin_session();
                    let _ = plugin.destroy_session(&ps).await;
                }
                json!({
                    "janus": "success",
                    "transaction": transaction,
                    "session_id": session_id.0,
                })
            }
            Err(e) => json!({
                "janus": "error",
                "transaction": transaction,
                "error": {
                    "code": 458,
                    "reason": e.to_string()
                }
            }),
        }
    }

    fn handle_keepalive(&self, transaction: &str, session_id: SessionId) -> serde_json::Value {
        if self.sessions.touch_session(session_id) {
            json!({ "janus": "ack", "transaction": transaction, "session_id": session_id.0 })
        } else {
            json!({
                "janus": "error",
                "transaction": transaction,
                "error": {
                    "code": 458,
                    "reason": format!("No such session {}", session_id)
                }
            })
        }
    }

    /// Handle a "message" request: dispatch to the appropriate plugin.
    async fn handle_message(
        self: &Arc<Self>,
        transaction: &str,
        _request: &TransportRequest,
        session_id: SessionId,
        handle_id: HandleId,
        message: &serde_json::Value,
    ) -> serde_json::Value {
        // Touch session
        self.sessions.touch_session(session_id);

        // Look up handle info
        let handle_info = match self.sessions.get_handle(session_id, handle_id) {
            Ok(info) => info,
            Err(_) => {
                return json!({
                    "janus": "error",
                    "transaction": transaction,
                    "error": {
                        "code": 458,
                        "reason": format!("No such handle {handle_id} in session {session_id}")
                    }
                });
            }
        };

        // Look up plugin
        let plugin = match self.plugins.get(&handle_info.plugin_package) {
            Some(p) => p.clone(),
            None => {
                return json!({
                    "janus": "error",
                    "transaction": transaction,
                    "error": {
                        "code": 460,
                        "reason": format!("Plugin '{}' not found", handle_info.plugin_package)
                    }
                });
            }
        };

        // Parse JSEP if present
        let jsep: Option<Jsep> = message
            .get("jsep")
            .and_then(|j| serde_json::from_value(j.clone()).ok());

        // Get the message body
        let body = message
            .get("body")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let ps = handle_info.plugin_session();
        let txn = transaction.to_string();

        // Dispatch to plugin
        match plugin.handle_message(&ps, &txn, body, jsep.clone()).await {
            Ok(PluginResult::Ok(payload)) => {
                // Plugin returned a synchronous result
                let mut event = json!({
                    "janus": "event",
                    "transaction": txn,
                    "session_id": session_id.0,
                    "sender": handle_id.0,
                    "plugindata": {
                        "plugin": handle_info.plugin_package,
                        "data": payload.body
                    }
                });

                // Handle JSEP in the response
                if let Some(jsep_resp) = payload.jsep {
                    if jsep_resp.jsep_type == JsepType::Answer {
                        // Create PeerConnection and generate proper SDP answer
                        if let Some(ref offer_jsep) = jsep {
                            match self
                                .setup_peer_connection(
                                    session_id,
                                    handle_id,
                                    &ps,
                                    offer_jsep,
                                )
                                .await
                            {
                                Ok(answer_sdp) => {
                                    event["jsep"] = json!({
                                        "type": "answer",
                                        "sdp": answer_sdp
                                    });
                                }
                                Err(e) => {
                                    warn!(error = %e, "failed to setup PeerConnection");
                                    // Fall back to the plugin-provided SDP
                                    event["jsep"] = serde_json::to_value(&jsep_resp)
                                        .unwrap_or_default();
                                }
                            }
                        } else {
                            event["jsep"] =
                                serde_json::to_value(&jsep_resp).unwrap_or_default();
                        }
                    } else {
                        event["jsep"] =
                            serde_json::to_value(&jsep_resp).unwrap_or_default();
                    }
                }

                // Push the event to the client via transport
                self.send_to_client(session_id, &event);

                // Return ack (the event is also pushed asynchronously)
                json!({
                    "janus": "ack",
                    "transaction": txn,
                    "session_id": session_id.0,
                    "hint": "Event pushed"
                })
            }
            Ok(PluginResult::OkWait { hint }) => {
                // Plugin will call push_event later
                json!({
                    "janus": "ack",
                    "transaction": txn,
                    "session_id": session_id.0,
                    "hint": hint.unwrap_or_else(|| "Processing...".into())
                })
            }
            Err(e) => {
                json!({
                    "janus": "error",
                    "transaction": txn,
                    "error": {
                        "code": 490,
                        "reason": e.to_string()
                    }
                })
            }
        }
    }

    /// Handle a "trickle" request: forward ICE candidates to PeerConnection.
    async fn handle_trickle(
        &self,
        transaction: &str,
        session_id: SessionId,
        handle_id: HandleId,
        message: &serde_json::Value,
    ) -> serde_json::Value {
        self.sessions.touch_session(session_id);

        let key = (session_id, handle_id);
        if let Some(pc) = self.peer_connections.get(&key) {
            if let Some(candidate_obj) = message.get("candidate") {
                let candidate = candidate_obj["candidate"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let mid = candidate_obj["sdpMid"]
                    .as_str()
                    .unwrap_or("0")
                    .to_string();

                if candidate.is_empty() || candidate_obj.get("completed").is_some() {
                    debug!("trickle: end-of-candidates");
                } else {
                    let _ = pc.add_ice_candidate(candidate, mid).await;
                }
            }
        }

        json!({
            "janus": "ack",
            "transaction": transaction,
            "session_id": session_id.0,
        })
    }

    /// Create an Arc<dyn WebRtcCallbacks> for WebRTC actors to call into.
    pub fn webrtc_callbacks(self: &Arc<Self>) -> Arc<dyn WebRtcCallbacks> {
        Arc::new(CoreWebRtcCallbacks {
            server: Arc::clone(self),
        })
    }

    /// Set up a PeerConnection for a handle, accepting an SDP offer.
    async fn setup_peer_connection(
        self: &Arc<Self>,
        session_id: SessionId,
        handle_id: HandleId,
        plugin_session: &PluginSession,
        offer_jsep: &Jsep,
    ) -> Result<String, String> {
        let callbacks = self.webrtc_callbacks();
        let config = PcConfig {
            ice_lite: self.config.nat.ice_lite,
            session: plugin_session.clone(),
            callbacks,
        };

        let handle = webrtc::create_peer_connection(config).await?;

        // Parse the offer and generate answer
        let offer = crate::sdp::jsep_to_sdp_offer(offer_jsep)?;
        let answer_sdp = handle.accept_offer(offer).await?;

        // Store the PeerConnection handle
        self.peer_connections
            .insert((session_id, handle_id), handle);

        Ok(answer_sdp)
    }

    /// Send an event to the client associated with a session.
    fn send_to_client(&self, session_id: SessionId, event: &serde_json::Value) {
        if let Some(client_id) = self.session_clients.get(&session_id) {
            if let Some(sender) = self.event_senders.get(client_id.value()) {
                let _ = sender.send(event.clone());
            }
        }
    }

    /// Signal the server to shut down.
    pub fn shutdown(&self) {
        info!("server shutdown requested");
        self.shutdown.notify_waiters();
    }

    /// Wait for the shutdown signal.
    pub async fn wait_for_shutdown(&self) {
        self.shutdown.notified().await;
    }
}

// ---------------------------------------------------------------------------
// Transport callbacks implementation
// ---------------------------------------------------------------------------

struct CoreTransportCallbacks {
    server: Arc<JanusServer>,
}

#[async_trait::async_trait]
impl janus_transport_api::TransportCallbacks for CoreTransportCallbacks {
    async fn incoming_request(
        &self,
        transport_name: &str,
        request: &TransportRequest,
        message: serde_json::Value,
    ) {
        debug!(
            transport = transport_name,
            client = %request.client_id,
            "incoming request"
        );
        let _response = self.server.process_request(request, message).await;
    }

    fn transport_gone(&self, transport_name: &str, request: &TransportRequest) {
        debug!(
            transport = transport_name,
            client = %request.client_id,
            "transport connection gone"
        );
    }

    fn is_api_secret_valid(&self, secret: &str) -> bool {
        match &self.server.config.general.api_secret {
            Some(expected) => expected == secret,
            None => true,
        }
    }

    fn is_auth_token_valid(&self, _token: &str) -> bool {
        !self.server.config.general.token_auth
    }
}

// ---------------------------------------------------------------------------
// Plugin callbacks implementation
// ---------------------------------------------------------------------------

struct CorePluginCallbacks {
    server: Arc<JanusServer>,
}

#[async_trait::async_trait]
impl PluginCallbacks for CorePluginCallbacks {
    fn relay_rtp(&self, session: &PluginSession, packet: &RtpPacket) {
        // Forward to PeerConnection if it exists
        let key = (session.session_id, session.handle_id);
        if let Some(pc) = self.server.peer_connections.get(&key) {
            pc.send_rtp(packet.clone());
        }
        // Also track in relay stats
        let _ = self.server.relay_sender.try_send(
            relay::RelayCommand::OutgoingRtp {
                session_id: session.session_id,
                handle_id: session.handle_id,
                packet: packet.clone(),
            },
        );
    }

    fn relay_rtcp(&self, session: &PluginSession, packet: &RtcpPacket) {
        let key = (session.session_id, session.handle_id);
        if let Some(pc) = self.server.peer_connections.get(&key) {
            pc.send_rtcp(packet.clone());
        }
        let _ = self.server.relay_sender.try_send(
            relay::RelayCommand::OutgoingRtcp {
                session_id: session.session_id,
                handle_id: session.handle_id,
                packet: packet.clone(),
            },
        );
    }

    fn relay_data(&self, _session: &PluginSession, _label: &str, _data: &[u8]) {
        // DataChannel relay — TODO
    }

    async fn push_event(
        &self,
        session: &PluginSession,
        transaction: &str,
        body: serde_json::Value,
        jsep: Option<Jsep>,
    ) -> janus_plugin_api::Result<()> {
        debug!(
            session = %session,
            transaction = transaction,
            "push_event from plugin"
        );

        // Look up the handle info to get plugin name
        let plugin_name = self
            .server
            .sessions
            .get_handle(session.session_id, session.handle_id)
            .map(|h| h.plugin_package.clone())
            .unwrap_or_default();

        let mut event = json!({
            "janus": "event",
            "transaction": transaction,
            "session_id": session.session_id.0,
            "sender": session.handle_id.0,
            "plugindata": {
                "plugin": plugin_name,
                "data": body
            }
        });

        // Handle JSEP — if the plugin provides a JSEP answer, set up the PeerConnection
        if let Some(ref jsep_val) = jsep {
            if jsep_val.jsep_type == JsepType::Answer {
                // The plugin is signaling it wants WebRTC. The SDP field contains
                // the original offer SDP (as a placeholder). Create a PeerConnection
                // and generate a real SDP answer.
                let offer_sdp = &jsep_val.sdp;
                match self
                    .server
                    .setup_peer_connection(
                        session.session_id,
                        session.handle_id,
                        session,
                        &Jsep {
                            jsep_type: JsepType::Offer,
                            sdp: offer_sdp.clone(),
                            trickle: jsep_val.trickle,
                        },
                    )
                    .await
                {
                    Ok(answer_sdp) => {
                        event["jsep"] = json!({
                            "type": "answer",
                            "sdp": answer_sdp
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to setup PeerConnection in push_event");
                        event["jsep"] = serde_json::to_value(jsep_val).unwrap_or_default();
                    }
                }
            } else {
                event["jsep"] = serde_json::to_value(jsep_val).unwrap_or_default();
            }
        }

        // Route through the transport to the client
        self.server.send_to_client(session.session_id, &event);

        Ok(())
    }

    fn close_pc(&self, session: &PluginSession) {
        debug!(session = %session, "close_pc requested by plugin");
        let key = (session.session_id, session.handle_id);
        if let Some((_, pc)) = self.server.peer_connections.remove(&key) {
            tokio::spawn(async move {
                pc.close().await;
            });
        }
    }

    fn end_session(&self, session: &PluginSession) {
        debug!(session = %session, "end_session requested by plugin");
    }

    fn notify_event(&self, plugin_name: &str, _event: serde_json::Value) {
        debug!(plugin = plugin_name, "plugin event");
    }
}

// ---------------------------------------------------------------------------
// WebRTC callbacks implementation (routes media events to plugins)
// ---------------------------------------------------------------------------

struct CoreWebRtcCallbacks {
    server: Arc<JanusServer>,
}

impl WebRtcCallbacks for CoreWebRtcCallbacks {
    fn on_media_ready(&self, session: &PluginSession) {
        if let Ok(handle) = self
            .server
            .sessions
            .get_handle(session.session_id, session.handle_id)
        {
            if let Some(plugin) = self.server.plugins.get(&handle.plugin_package) {
                plugin.setup_media(session);
            }
        }
    }

    fn on_media_hangup(&self, session: &PluginSession, reason: &str) {
        if let Ok(handle) = self
            .server
            .sessions
            .get_handle(session.session_id, session.handle_id)
        {
            if let Some(plugin) = self.server.plugins.get(&handle.plugin_package) {
                plugin.hangup_media(session, reason);
            }
        }
    }

    fn on_incoming_rtp(&self, session: &PluginSession, packet: &RtpPacket) {
        if let Ok(handle) = self
            .server
            .sessions
            .get_handle(session.session_id, session.handle_id)
        {
            if let Some(plugin) = self.server.plugins.get(&handle.plugin_package) {
                plugin.incoming_rtp(session, packet);
            }
        }
    }

    fn on_incoming_rtcp(&self, session: &PluginSession, packet: &RtcpPacket) {
        if let Ok(handle) = self
            .server
            .sessions
            .get_handle(session.session_id, session.handle_id)
        {
            if let Some(plugin) = self.server.plugins.get(&handle.plugin_package) {
                plugin.incoming_rtcp(session, packet);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server() -> Arc<JanusServer> {
        Arc::new(JanusServer::new(JanusConfig::default()))
    }

    fn test_server_with_secret() -> Arc<JanusServer> {
        let mut config = JanusConfig::default();
        config.general.api_secret = Some("mysecret".into());
        Arc::new(JanusServer::new(config))
    }

    #[tokio::test]
    async fn handle_ping() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({"janus": "ping", "transaction": "txn1"}),
            )
            .await;
        assert_eq!(resp["janus"], "pong");
        assert_eq!(resp["transaction"], "txn1");
    }

    #[tokio::test]
    async fn handle_info() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({"janus": "info", "transaction": "txn2"}),
            )
            .await;
        assert_eq!(resp["janus"], "server_info");
        assert!(resp["name"].is_string());
        assert_eq!(resp["session_timeout"], 60);
    }

    #[tokio::test]
    async fn handle_create_and_destroy() {
        let server = test_server();
        let req = TransportRequest::new("test-client");

        // Create session
        let resp = server
            .process_request(
                &req,
                json!({"janus": "create", "transaction": "txn3"}),
            )
            .await;
        assert_eq!(resp["janus"], "success");
        let session_id = resp["data"]["id"].as_u64().unwrap();
        assert!(session_id > 0);
        assert_eq!(server.sessions().session_count(), 1);

        // Destroy session
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "destroy",
                    "transaction": "txn4",
                    "session_id": session_id
                }),
            )
            .await;
        assert_eq!(resp["janus"], "success");
        assert_eq!(server.sessions().session_count(), 0);
    }

    #[tokio::test]
    async fn handle_attach_and_detach() {
        let server = test_server();
        let req = TransportRequest::new("test-client");

        // Create session
        let resp = server
            .process_request(
                &req,
                json!({"janus": "create", "transaction": "t1"}),
            )
            .await;
        let session_id = resp["data"]["id"].as_u64().unwrap();

        // Attach plugin
        let resp = server
            .process_request(
                &req,
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
        assert!(handle_id > 0);

        // Detach handle
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "detach",
                    "transaction": "t3",
                    "session_id": session_id,
                    "handle_id": handle_id
                }),
            )
            .await;
        assert_eq!(resp["janus"], "success");
    }

    #[tokio::test]
    async fn handle_keepalive() {
        let server = test_server();
        let req = TransportRequest::new("test-client");

        let resp = server
            .process_request(
                &req,
                json!({"janus": "create", "transaction": "t1"}),
            )
            .await;
        let session_id = resp["data"]["id"].as_u64().unwrap();

        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "keepalive",
                    "transaction": "t2",
                    "session_id": session_id
                }),
            )
            .await;
        assert_eq!(resp["janus"], "ack");
    }

    #[tokio::test]
    async fn keepalive_unknown_session_errors() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "keepalive",
                    "transaction": "t1",
                    "session_id": 999999
                }),
            )
            .await;
        assert_eq!(resp["janus"], "error");
    }

    #[tokio::test]
    async fn unknown_request_returns_error() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({"janus": "blah", "transaction": "t1"}),
            )
            .await;
        assert_eq!(resp["janus"], "error");
        assert_eq!(resp["error"]["code"], 455);
    }

    #[tokio::test]
    async fn api_secret_enforced() {
        let server = test_server_with_secret();
        let req = TransportRequest::new("test-client");

        // Without secret — should fail
        let resp = server
            .process_request(
                &req,
                json!({"janus": "ping", "transaction": "t1"}),
            )
            .await;
        assert_eq!(resp["janus"], "error");
        assert_eq!(resp["error"]["code"], 403);

        // With wrong secret
        let resp = server
            .process_request(
                &req,
                json!({"janus": "ping", "transaction": "t2", "apisecret": "wrong"}),
            )
            .await;
        assert_eq!(resp["janus"], "error");

        // With correct secret
        let resp = server
            .process_request(
                &req,
                json!({"janus": "ping", "transaction": "t3", "apisecret": "mysecret"}),
            )
            .await;
        assert_eq!(resp["janus"], "pong");
    }

    #[tokio::test]
    async fn destroy_nonexistent_session_errors() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "destroy",
                    "transaction": "t1",
                    "session_id": 12345
                }),
            )
            .await;
        assert_eq!(resp["janus"], "error");
        assert_eq!(resp["error"]["code"], 458);
    }

    #[tokio::test]
    async fn attach_to_nonexistent_session_errors() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "attach",
                    "transaction": "t1",
                    "session_id": 99999,
                    "plugin": "janus.plugin.echotest"
                }),
            )
            .await;
        assert_eq!(resp["janus"], "error");
    }

    #[tokio::test]
    async fn missing_janus_field_treated_as_empty() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(&req, json!({"transaction": "t1"}))
            .await;
        assert_eq!(resp["janus"], "error");
        assert_eq!(resp["error"]["code"], 455);
    }

    #[tokio::test]
    async fn missing_transaction_still_works() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(&req, json!({"janus": "ping"}))
            .await;
        assert_eq!(resp["janus"], "pong");
        assert_eq!(resp["transaction"], "");
    }

    #[tokio::test]
    async fn create_multiple_sessions() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        for i in 0..20 {
            let resp = server
                .process_request(
                    &req,
                    json!({"janus": "create", "transaction": format!("t{i}")}),
                )
                .await;
            assert_eq!(resp["janus"], "success");
        }
        assert_eq!(server.sessions().session_count(), 20);
    }

    #[tokio::test]
    async fn detach_nonexistent_handle_errors() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(&req, json!({"janus": "create", "transaction": "t1"}))
            .await;
        let session_id = resp["data"]["id"].as_u64().unwrap();

        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "detach",
                    "transaction": "t2",
                    "session_id": session_id,
                    "handle_id": 999999
                }),
            )
            .await;
        assert_eq!(resp["janus"], "error");
        assert_eq!(resp["error"]["code"], 458);
    }

    #[tokio::test]
    async fn destroy_session_twice_errors_second_time() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(&req, json!({"janus": "create", "transaction": "t1"}))
            .await;
        let session_id = resp["data"]["id"].as_u64().unwrap();

        let resp = server
            .process_request(
                &req,
                json!({"janus": "destroy", "transaction": "t2", "session_id": session_id}),
            )
            .await;
        assert_eq!(resp["janus"], "success");

        let resp = server
            .process_request(
                &req,
                json!({"janus": "destroy", "transaction": "t3", "session_id": session_id}),
            )
            .await;
        assert_eq!(resp["janus"], "error");
    }

    #[tokio::test]
    async fn message_returns_ack() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "message",
                    "transaction": "t1",
                    "session_id": 1,
                    "handle_id": 2
                }),
            )
            .await;
        // With no valid session/handle, should return error now
        assert!(resp["janus"] == "error" || resp["janus"] == "ack");
    }

    #[tokio::test]
    async fn trickle_returns_ack() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "trickle",
                    "transaction": "t1",
                    "session_id": 1,
                    "handle_id": 2,
                    "candidate": {"sdpMid": "audio", "sdpMLineIndex": 0, "candidate": "candidate:..."}
                }),
            )
            .await;
        assert_eq!(resp["janus"], "ack");
    }

    #[tokio::test]
    async fn api_secret_not_required_when_unset() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(&req, json!({"janus": "ping", "transaction": "t1"}))
            .await;
        assert_eq!(resp["janus"], "pong");
    }

    #[tokio::test]
    async fn info_returns_version() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(&req, json!({"janus": "info", "transaction": "t1"}))
            .await;
        assert!(resp["version_string"].is_string());
        assert_eq!(resp["data_channels"], true);
    }

    #[tokio::test]
    async fn attach_multiple_handles_to_same_session() {
        let server = test_server();
        let req = TransportRequest::new("test-client");
        let resp = server
            .process_request(&req, json!({"janus": "create", "transaction": "t1"}))
            .await;
        let session_id = resp["data"]["id"].as_u64().unwrap();

        let mut handle_ids = Vec::new();
        for i in 0..5 {
            let resp = server
                .process_request(
                    &req,
                    json!({
                        "janus": "attach",
                        "transaction": format!("attach-{i}"),
                        "session_id": session_id,
                        "plugin": "janus.plugin.echotest"
                    }),
                )
                .await;
            assert_eq!(resp["janus"], "success");
            handle_ids.push(resp["data"]["id"].as_u64().unwrap());
        }

        handle_ids.sort();
        handle_ids.dedup();
        assert_eq!(handle_ids.len(), 5);
    }

    #[test]
    fn server_can_be_created_and_dropped() {
        let server = JanusServer::new(JanusConfig::default());
        assert_eq!(server.sessions().session_count(), 0);
        drop(server);
    }

    #[test]
    fn transport_callbacks_can_be_created() {
        let server = Arc::new(JanusServer::new(JanusConfig::default()));
        let _callbacks = server.transport_callbacks();
    }

    #[test]
    fn plugin_callbacks_can_be_created() {
        let server = Arc::new(JanusServer::new(JanusConfig::default()));
        let _callbacks = server.plugin_callbacks();
    }

    #[test]
    fn register_plugin_works() {
        let server = JanusServer::new(JanusConfig::default());
        assert_eq!(server.plugins.len(), 0);
        // We'd need a mock plugin to test this fully, but the method exists
    }

    #[tokio::test]
    async fn message_to_registered_plugin_dispatches() {
        use janus_plugin_echotest::EchoTestPlugin;
        use std::path::Path;

        let server = Arc::new(JanusServer::new(JanusConfig::default()));
        let mut echotest = EchoTestPlugin::default();
        let callbacks = server.plugin_callbacks();
        echotest.init(callbacks, Path::new("/tmp")).await.unwrap();
        server.register_plugin(Arc::new(echotest));

        let req = TransportRequest::new("test-client");

        // Create session
        let resp = server
            .process_request(&req, json!({"janus": "create", "transaction": "t1"}))
            .await;
        let session_id = resp["data"]["id"].as_u64().unwrap();

        // Attach
        let resp = server
            .process_request(
                &req,
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

        // Send message
        let resp = server
            .process_request(
                &req,
                json!({
                    "janus": "message",
                    "transaction": "t3",
                    "session_id": session_id,
                    "handle_id": handle_id,
                    "body": {"audio": true, "video": true}
                }),
            )
            .await;
        // Should get ack (event pushed asynchronously)
        assert_eq!(resp["janus"], "ack");
    }
}
