//! Janus Streaming plugin — serve RTP sources to WebRTC viewers.
//!
//! This plugin allows creating streaming mountpoints that receive RTP from
//! external sources (e.g., ffmpeg, GStreamer) and relay them to WebRTC viewers.

pub mod mountpoint;

use crate::mountpoint::{MountpointConfig, MountpointId, MountpointRegistry, ViewerInfo};
use async_trait::async_trait;
use dashmap::DashMap;
use janus_plugin_api::{
    HandleId, Jsep, JsepType, PluginCallbacks, PluginResult, PluginResultPayload, PluginSession,
    RtpPacket, SessionId,
};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Per-handle state tracking.
#[derive(Debug)]
enum HandleState {
    /// No state yet (handle just created).
    None,
    /// Viewer watching a mountpoint.
    Viewer {
        mountpoint_id: MountpointId,
        started: bool,
    },
}

/// The Streaming plugin.
pub struct StreamingPlugin {
    mountpoints: MountpointRegistry,
    /// Map from (session_id, handle_id) to per-handle state.
    handle_states: DashMap<(SessionId, HandleId), HandleState>,
    /// Plugin callbacks for communicating with the core.
    callbacks: Option<Arc<dyn PluginCallbacks>>,
}

impl Default for StreamingPlugin {
    fn default() -> Self {
        Self {
            mountpoints: MountpointRegistry::new(),
            handle_states: DashMap::new(),
            callbacks: None,
        }
    }
}

impl std::fmt::Debug for StreamingPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingPlugin")
            .field("mountpoints", &self.mountpoints.len())
            .field("handles", &self.handle_states.len())
            .finish()
    }
}

#[async_trait]
impl janus_plugin_api::JanusPlugin for StreamingPlugin {
    fn name(&self) -> &'static str {
        "Janus Streaming plugin"
    }

    fn package(&self) -> &'static str {
        "janus.plugin.streaming"
    }

    fn version(&self) -> u32 {
        1
    }

    fn version_string(&self) -> &'static str {
        "0.1.0"
    }

    fn description(&self) -> &'static str {
        "Serves RTP sources to WebRTC viewers (Rust implementation)"
    }

    fn author(&self) -> &'static str {
        "Janus Rust Contributors"
    }

    async fn init(
        &mut self,
        callbacks: Arc<dyn PluginCallbacks>,
        _config_path: &Path,
    ) -> janus_plugin_api::Result<()> {
        info!("Streaming plugin initializing...");
        self.callbacks = Some(callbacks);

        // Create a default mountpoint (ID=1) for testing
        let config = MountpointConfig {
            name: "Test Stream".into(),
            description: "Default test stream (mountpoint 1)".into(),
            audio_port: 5004,
            video_port: 5006,
            ..Default::default()
        };
        self.mountpoints.create(Some(1), config);
        info!(mountpoint_id = 1, "created default streaming mountpoint");

        info!("Streaming plugin initialized");
        Ok(())
    }

    async fn destroy(&mut self) -> janus_plugin_api::Result<()> {
        info!("Streaming plugin destroying...");
        self.handle_states.clear();
        self.callbacks = None;
        Ok(())
    }

    async fn create_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let key = (session.session_id, session.handle_id);
        debug!(session = %session, "streaming: new session");
        self.handle_states.insert(key, HandleState::None);
        Ok(())
    }

    async fn destroy_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let key = (session.session_id, session.handle_id);
        debug!(session = %session, "streaming: destroying session");

        // Remove viewer from mountpoint if watching
        if let Some((_, state)) = self.handle_states.remove(&key) {
            if let HandleState::Viewer { mountpoint_id, .. } = state {
                if let Some(mp) = self.mountpoints.get(mountpoint_id) {
                    mp.viewers.remove(&session.handle_id);
                }
            }
        }

        Ok(())
    }

    fn query_session(&self, session: &PluginSession) -> janus_plugin_api::Result<serde_json::Value> {
        let key = (session.session_id, session.handle_id);
        if let Some(state) = self.handle_states.get(&key) {
            match &*state {
                HandleState::None => Ok(json!({"state": "none"})),
                HandleState::Viewer {
                    mountpoint_id,
                    started,
                } => Ok(json!({
                    "state": "viewer",
                    "mountpoint_id": mountpoint_id,
                    "started": started,
                })),
            }
        } else {
            Err(janus_plugin_api::Error::SessionNotFound(session.to_string()))
        }
    }

    async fn handle_message(
        &self,
        session: &PluginSession,
        transaction: &str,
        body: serde_json::Value,
        jsep: Option<Jsep>,
    ) -> janus_plugin_api::Result<PluginResult> {
        let key = (session.session_id, session.handle_id);
        let request = body["request"]
            .as_str()
            .ok_or_else(|| janus_plugin_api::Error::MissingField("request".into()))?;

        debug!(session = %session, request, "streaming: handle_message");

        match request {
            // ── List all mountpoints ──
            "list" => {
                let list: Vec<serde_json::Value> = self
                    .mountpoints
                    .list()
                    .iter()
                    .filter(|mp| !mp.config.is_private)
                    .map(|mp| {
                        json!({
                            "id": mp.id,
                            "type": "rtp",
                            "description": mp.config.description,
                            "audio_age_ms": 0,
                            "video_age_ms": 0,
                        })
                    })
                    .collect();

                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({
                        "streaming": "list",
                        "list": list,
                    }),
                    jsep: None,
                }))
            }

            // ── Mountpoint info ──
            "info" => {
                let id = body["id"]
                    .as_u64()
                    .ok_or_else(|| janus_plugin_api::Error::MissingField("id".into()))?;

                let mp = self
                    .mountpoints
                    .get(id)
                    .ok_or_else(|| {
                        janus_plugin_api::Error::Plugin(format!("No such mountpoint: {id}"))
                    })?;

                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({
                        "streaming": "info",
                        "info": {
                            "id": mp.id,
                            "name": mp.config.name,
                            "description": mp.config.description,
                            "type": "rtp",
                            "audio_port": mp.config.audio_port,
                            "video_port": mp.config.video_port,
                            "viewers": mp.viewers.len(),
                        }
                    }),
                    jsep: None,
                }))
            }

            // ── Create a mountpoint ──
            "create" => {
                let mp_id = body["id"].as_u64();
                if let Some(id) = mp_id {
                    if self.mountpoints.exists(id) {
                        return Err(janus_plugin_api::Error::Plugin(format!(
                            "Mountpoint {id} already exists"
                        )));
                    }
                }

                let config = MountpointConfig {
                    name: body["name"].as_str().unwrap_or("Stream").to_string(),
                    description: body["description"]
                        .as_str()
                        .unwrap_or("Streaming mountpoint")
                        .to_string(),
                    is_private: body["is_private"].as_bool().unwrap_or(false),
                    pin: body["pin"].as_str().map(|s| s.to_string()),
                    secret: body["secret"].as_str().map(|s| s.to_string()),
                    audio_port: body["audio_port"].as_u64().unwrap_or(0) as u16,
                    video_port: body["video_port"].as_u64().unwrap_or(0) as u16,
                    audio_codec: body["audio_codec"]
                        .as_str()
                        .map(|s| s.to_string())
                        .or(Some("opus".into())),
                    video_codec: body["video_codec"]
                        .as_str()
                        .map(|s| s.to_string())
                        .or(Some("vp8".into())),
                    audio_pt: body["audio_pt"].as_u64().map(|v| v as u8).or(Some(111)),
                    video_pt: body["video_pt"].as_u64().map(|v| v as u8).or(Some(96)),
                };

                let mp = self.mountpoints.create(mp_id, config);
                info!(mountpoint_id = mp.id, "streaming: created mountpoint");

                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({
                        "streaming": "created",
                        "created": mp.id,
                    }),
                    jsep: None,
                }))
            }

            // ── Destroy a mountpoint ──
            "destroy" => {
                let id = body["id"]
                    .as_u64()
                    .ok_or_else(|| janus_plugin_api::Error::MissingField("id".into()))?;

                if let Some(secret) = body["secret"].as_str() {
                    if let Some(mp) = self.mountpoints.get(id) {
                        if let Some(ref mp_secret) = mp.config.secret {
                            if mp_secret != secret {
                                return Err(janus_plugin_api::Error::Plugin(
                                    "Unauthorized (wrong secret)".into(),
                                ));
                            }
                        }
                    }
                }

                self.mountpoints.destroy(id).ok_or_else(|| {
                    janus_plugin_api::Error::Plugin(format!("No such mountpoint: {id}"))
                })?;

                info!(mountpoint_id = id, "streaming: destroyed mountpoint");

                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({
                        "streaming": "destroyed",
                        "destroyed": id,
                    }),
                    jsep: None,
                }))
            }

            // ── Watch a mountpoint ──
            "watch" => {
                let mp_id = body["id"]
                    .as_u64()
                    .ok_or_else(|| janus_plugin_api::Error::MissingField("id".into()))?;

                let mp = self.mountpoints.get(mp_id).ok_or_else(|| {
                    janus_plugin_api::Error::Plugin(format!("No such mountpoint: {mp_id}"))
                })?;

                // Check PIN if required
                if let Some(ref mp_pin) = mp.config.pin {
                    let provided = body["pin"].as_str().unwrap_or("");
                    if provided != mp_pin {
                        return Err(janus_plugin_api::Error::Plugin(
                            "Unauthorized (wrong pin)".into(),
                        ));
                    }
                }

                // Register viewer
                mp.viewers.insert(
                    session.handle_id,
                    ViewerInfo {
                        handle_id: session.handle_id,
                        paused: false,
                    },
                );

                // Update handle state
                self.handle_states.insert(
                    key,
                    HandleState::Viewer {
                        mountpoint_id: mp_id,
                        started: false,
                    },
                );

                info!(
                    session = %session,
                    mountpoint_id = mp_id,
                    "streaming: viewer watching"
                );

                // Generate a placeholder SDP offer based on mountpoint config.
                // In production the core replaces this with a real str0m offer.
                let offer_sdp = format!(
                    "v=0\r\n\
                     o=- 0 0 IN IP4 127.0.0.1\r\n\
                     s=Streaming {mp_id}\r\n\
                     t=0 0\r\n\
                     m=audio 9 UDP/TLS/RTP/SAVPF {apt}\r\n\
                     a=recvonly\r\n\
                     m=video 9 UDP/TLS/RTP/SAVPF {vpt}\r\n\
                     a=recvonly\r\n",
                    apt = mp.config.audio_pt.unwrap_or(111),
                    vpt = mp.config.video_pt.unwrap_or(96),
                );

                // Push event with JSEP offer via callbacks (async)
                if let Some(ref callbacks) = self.callbacks {
                    let offer_jsep = Jsep {
                        jsep_type: JsepType::Offer,
                        sdp: offer_sdp,
                        trickle: false,
                    };
                    let event_body = json!({
                        "streaming": "event",
                        "result": {
                            "status": "preparing",
                        }
                    });
                    let _ = callbacks
                        .push_event(session, transaction, event_body, Some(offer_jsep))
                        .await;
                }

                Ok(PluginResult::OkWait {
                    hint: Some("Preparing stream".into()),
                })
            }

            // ── Start receiving media ──
            "start" => {
                let mut state = self
                    .handle_states
                    .get_mut(&key)
                    .ok_or_else(|| {
                        janus_plugin_api::Error::SessionNotFound(session.to_string())
                    })?;

                match &mut *state {
                    HandleState::Viewer {
                        mountpoint_id,
                        started,
                    } => {
                        *started = true;
                        let mp_id = *mountpoint_id;

                        info!(
                            session = %session,
                            mountpoint_id = mp_id,
                            "streaming: viewer started"
                        );

                        // If the viewer sent a JSEP answer, the core handles it
                        if let Some(ref _answer) = jsep {
                            // The core's push_event handler will process the JSEP answer
                            // and set up the PeerConnection
                        }

                        Ok(PluginResult::Ok(PluginResultPayload {
                            body: json!({
                                "streaming": "event",
                                "result": {
                                    "status": "started",
                                }
                            }),
                            jsep: None,
                        }))
                    }
                    _ => Err(janus_plugin_api::Error::Plugin("Not a viewer".into())),
                }
            }

            // ── Pause media ──
            "pause" => {
                let state = self.handle_states.get(&key).ok_or_else(|| {
                    janus_plugin_api::Error::SessionNotFound(session.to_string())
                })?;

                if let HandleState::Viewer { mountpoint_id, .. } = &*state {
                    if let Some(mp) = self.mountpoints.get(*mountpoint_id) {
                        if let Some(mut viewer) = mp.viewers.get_mut(&session.handle_id) {
                            viewer.paused = true;
                        }
                    }
                    Ok(PluginResult::Ok(PluginResultPayload {
                        body: json!({
                            "streaming": "event",
                            "result": {
                                "status": "pausing",
                            }
                        }),
                        jsep: None,
                    }))
                } else {
                    Err(janus_plugin_api::Error::Plugin("Not a viewer".into()))
                }
            }

            // ── Stop watching ──
            "stop" => {
                if let Some((_, state)) = self.handle_states.remove(&key) {
                    if let HandleState::Viewer { mountpoint_id, .. } = state {
                        if let Some(mp) = self.mountpoints.get(mountpoint_id) {
                            mp.viewers.remove(&session.handle_id);
                        }
                    }
                }
                self.handle_states.insert(key, HandleState::None);

                info!(session = %session, "streaming: viewer stopped");

                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({
                        "streaming": "event",
                        "result": {
                            "status": "stopped",
                        }
                    }),
                    jsep: None,
                }))
            }

            _ => Err(janus_plugin_api::Error::InvalidRequest(format!(
                "Unknown request: {request}"
            ))),
        }
    }

    fn incoming_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {
        // Streaming plugin receives RTP from external UDP sources, not from
        // WebRTC peers. This method would be called if a viewer somehow sent
        // RTP back, which we ignore.
    }

    fn slow_link(&self, session: &PluginSession, uplink: bool, lost: u32) {
        warn!(
            session = %session,
            uplink = uplink,
            lost_packets = lost,
            "streaming slow link"
        );
    }
}

impl StreamingPlugin {
    /// Relay an incoming RTP packet to all active viewers of a mountpoint.
    /// Called by the RTP listener task when data arrives on a mountpoint's UDP port.
    pub fn relay_to_viewers(&self, mountpoint_id: MountpointId, packet: &RtpPacket) {
        let mp = match self.mountpoints.get(mountpoint_id) {
            Some(mp) => mp,
            None => return,
        };

        let callbacks = match &self.callbacks {
            Some(cb) => cb,
            None => return,
        };

        for viewer in mp.viewers.iter() {
            if viewer.paused {
                continue;
            }

            let viewer_handle_id = viewer.handle_id;
            // Find the viewer's state to check if started
            // We iterate handle_states to find the matching handle_id
            let mut is_started = false;
            for entry in self.handle_states.iter() {
                let ((_sid, hid), state) = entry.pair();
                if *hid == viewer_handle_id {
                    if let HandleState::Viewer { started: true, .. } = state {
                        is_started = true;
                    }
                    break;
                }
            }

            if is_started {
                // We need a PluginSession to relay. Find it from the handle_states keys.
                for entry in self.handle_states.iter() {
                    let ((sid, hid), _) = entry.pair();
                    if *hid == viewer_handle_id {
                        let viewer_session = PluginSession::new(*sid, *hid);
                        callbacks.relay_rtp(&viewer_session, packet);
                        break;
                    }
                }
            }
        }
    }

    /// Get the mountpoint registry (for external RTP listener integration).
    pub fn mountpoints(&self) -> &MountpointRegistry {
        &self.mountpoints
    }
}

/// FFI entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_streaming_create() -> *mut dyn janus_plugin_api::JanusPlugin {
    let plugin = StreamingPlugin::default();
    Box::into_raw(Box::new(plugin))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use janus_plugin_api::{JanusPlugin, RtcpPacket};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct MockCallbacks {
        rtp_relayed: AtomicU64,
        events_pushed: AtomicU64,
    }

    impl MockCallbacks {
        fn new() -> Self {
            Self {
                rtp_relayed: AtomicU64::new(0),
                events_pushed: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl PluginCallbacks for MockCallbacks {
        fn relay_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {
            self.rtp_relayed.fetch_add(1, Ordering::Relaxed);
        }

        fn relay_rtcp(&self, _session: &PluginSession, _packet: &RtcpPacket) {}

        fn relay_data(&self, _session: &PluginSession, _label: &str, _data: &[u8]) {}

        async fn push_event(
            &self,
            _session: &PluginSession,
            _transaction: &str,
            _body: serde_json::Value,
            _jsep: Option<Jsep>,
        ) -> janus_plugin_api::Result<()> {
            self.events_pushed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn close_pc(&self, _session: &PluginSession) {}
        fn end_session(&self, _session: &PluginSession) {}
        fn notify_event(&self, _plugin_name: &str, _event: serde_json::Value) {}
    }

    fn mock_session(id: u64) -> PluginSession {
        PluginSession::new(SessionId(1), HandleId(id))
    }

    // ── Metadata ──

    #[test]
    fn metadata() {
        let plugin = StreamingPlugin::default();
        assert_eq!(plugin.package(), "janus.plugin.streaming");
        assert_eq!(plugin.version(), 1);
        assert!(!plugin.name().is_empty());
        assert!(!plugin.description().is_empty());
        assert!(!plugin.author().is_empty());
    }

    // ── Init ──

    #[tokio::test]
    async fn init_creates_default_mountpoint() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        // Default mountpoint 1 should exist
        assert!(plugin.mountpoints.exists(1));
        let mp = plugin.mountpoints.get(1).unwrap();
        assert_eq!(mp.config.audio_port, 5004);
        assert_eq!(mp.config.video_port, 5006);
    }

    // ── Create mountpoint ──

    #[tokio::test]
    async fn create_mountpoint() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn1",
                json!({
                    "request": "create",
                    "id": 42,
                    "name": "My Stream",
                    "audio_port": 6000,
                    "video_port": 6002,
                }),
                None,
            )
            .await
            .unwrap();

        if let PluginResult::Ok(payload) = result {
            assert_eq!(payload.body["streaming"], "created");
            assert_eq!(payload.body["created"], 42);
        } else {
            panic!("expected Ok result");
        }

        assert!(plugin.mountpoints.exists(42));
    }

    // ── Destroy mountpoint ──

    #[tokio::test]
    async fn destroy_mountpoint() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(1);
        plugin.create_session(&session).await.unwrap();

        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "create", "id": 50}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn2",
                json!({"request": "destroy", "id": 50}),
                None,
            )
            .await
            .unwrap();

        if let PluginResult::Ok(payload) = result {
            assert_eq!(payload.body["streaming"], "destroyed");
        } else {
            panic!("expected Ok result");
        }

        assert!(!plugin.mountpoints.exists(50));
    }

    // ── List mountpoints ──

    #[tokio::test]
    async fn list_mountpoints() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(&session, "txn1", json!({"request": "list"}), None)
            .await
            .unwrap();

        if let PluginResult::Ok(payload) = result {
            assert_eq!(payload.body["streaming"], "list");
            let list = payload.body["list"].as_array().unwrap();
            assert!(!list.is_empty());
        } else {
            panic!("expected Ok result");
        }
    }

    // ── Info ──

    #[tokio::test]
    async fn mountpoint_info() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "info", "id": 1}),
                None,
            )
            .await
            .unwrap();

        if let PluginResult::Ok(payload) = result {
            assert_eq!(payload.body["streaming"], "info");
            assert_eq!(payload.body["info"]["id"], 1);
        } else {
            panic!("expected Ok result");
        }
    }

    // ── Watch ──

    #[tokio::test]
    async fn watch_mountpoint() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();

        assert!(matches!(result, PluginResult::OkWait { .. }));

        // Viewer should be registered
        let mp = plugin.mountpoints.get(1).unwrap();
        assert_eq!(mp.viewers.len(), 1);
    }

    // ── Start ──

    #[tokio::test]
    async fn start_viewer() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        // Watch first
        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();

        // Then start
        let result = plugin
            .handle_message(
                &session,
                "txn2",
                json!({"request": "start"}),
                Some(Jsep {
                    jsep_type: JsepType::Answer,
                    sdp: "v=0\r\n".into(),
                    trickle: true,
                }),
            )
            .await
            .unwrap();

        if let PluginResult::Ok(payload) = result {
            assert_eq!(payload.body["streaming"], "event");
            assert_eq!(payload.body["result"]["status"], "started");
        } else {
            panic!("expected Ok result");
        }
    }

    // ── Pause ──

    #[tokio::test]
    async fn pause_viewer() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn2",
                json!({"request": "pause"}),
                None,
            )
            .await
            .unwrap();

        if let PluginResult::Ok(payload) = result {
            assert_eq!(payload.body["result"]["status"], "pausing");
        } else {
            panic!("expected Ok result");
        }
    }

    // ── Stop ──

    #[tokio::test]
    async fn stop_viewer() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn2",
                json!({"request": "stop"}),
                None,
            )
            .await
            .unwrap();

        if let PluginResult::Ok(payload) = result {
            assert_eq!(payload.body["result"]["status"], "stopped");
        } else {
            panic!("expected Ok result");
        }

        let mp = plugin.mountpoints.get(1).unwrap();
        assert_eq!(mp.viewers.len(), 0);
    }

    // ── RTP relay to viewers ──

    #[tokio::test]
    async fn relay_to_viewers() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_ref = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        // Watch and start
        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();
        let _ = plugin
            .handle_message(
                &session,
                "txn2",
                json!({"request": "start"}),
                None,
            )
            .await
            .unwrap();

        let packet = RtpPacket {
            video: true,
            buffer: vec![
                0x80, 0x60, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            ],
        };
        plugin.relay_to_viewers(1, &packet);

        assert_eq!(cb_ref.rtp_relayed.load(Ordering::Relaxed), 1);
    }

    // ── Paused viewer doesn't get RTP ──

    #[tokio::test]
    async fn paused_viewer_no_rtp() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_ref = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();
        let _ = plugin
            .handle_message(
                &session,
                "txn2",
                json!({"request": "start"}),
                None,
            )
            .await
            .unwrap();
        let _ = plugin
            .handle_message(
                &session,
                "txn3",
                json!({"request": "pause"}),
                None,
            )
            .await
            .unwrap();

        let packet = RtpPacket {
            video: true,
            buffer: vec![0x80, 0x60, 0x00, 0x01],
        };
        plugin.relay_to_viewers(1, &packet);

        assert_eq!(cb_ref.rtp_relayed.load(Ordering::Relaxed), 0);
    }

    // ── Destroy session cleans up viewer ──

    #[tokio::test]
    async fn destroy_session_cleanup() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();

        assert_eq!(plugin.mountpoints.get(1).unwrap().viewers.len(), 1);

        plugin.destroy_session(&session).await.unwrap();

        assert_eq!(plugin.mountpoints.get(1).unwrap().viewers.len(), 0);
    }

    // ── Unknown request ──

    #[tokio::test]
    async fn unknown_request_errors() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "invalid"}),
                None,
            )
            .await;

        assert!(result.is_err());
    }

    // ── Nonexistent mountpoint ──

    #[tokio::test]
    async fn watch_nonexistent_mountpoint_errors() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 9999}),
                None,
            )
            .await;

        assert!(result.is_err());
    }

    // ── Query session ──

    #[tokio::test]
    async fn query_session_viewer() {
        let mut plugin = StreamingPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = mock_session(100);
        plugin.create_session(&session).await.unwrap();

        // Before watch
        let info = plugin.query_session(&session).unwrap();
        assert_eq!(info["state"], "none");

        // After watch
        let _ = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"request": "watch", "id": 1}),
                None,
            )
            .await
            .unwrap();

        let info = plugin.query_session(&session).unwrap();
        assert_eq!(info["state"], "viewer");
        assert_eq!(info["mountpoint_id"], 1);
    }

    // ── FFI ──

    #[test]
    fn ffi_create_returns_valid_plugin() {
        let raw = janus_plugin_streaming_create();
        assert!(!raw.is_null());
        unsafe {
            let _ = Box::from_raw(raw);
        }
    }
}
