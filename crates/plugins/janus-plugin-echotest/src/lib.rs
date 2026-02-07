//! Janus EchoTest plugin.
//!
//! A simple plugin that echoes RTP audio/video back to the sender.
//! Supports toggling audio/video and setting a bitrate cap.
//! This serves as the reference implementation for the plugin API.

use async_trait::async_trait;
use dashmap::DashMap;
use janus_plugin_api::{
    HandleId, Jsep, JsepType, PluginCallbacks, PluginResult, PluginResultPayload, PluginSession,
    RtcpPacket, RtpPacket, SessionId,
};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Per-session state for the echotest.
#[derive(Debug)]
struct EchoSession {
    /// Whether audio echoing is enabled.
    audio: bool,
    /// Whether video echoing is enabled.
    video: bool,
    /// Bitrate cap in bps (0 = no cap).
    bitrate: u64,
    /// Whether this session is actively relaying media.
    active: bool,
}

/// The EchoTest plugin.
pub struct EchoTestPlugin {
    callbacks: Option<Arc<dyn PluginCallbacks>>,
    sessions: DashMap<(SessionId, HandleId), EchoSession>,
}

impl Default for EchoTestPlugin {
    fn default() -> Self {
        Self {
            callbacks: None,
            sessions: DashMap::new(),
        }
    }
}

/// Incoming message body from the client.
#[derive(Debug, Deserialize)]
struct EchoTestMessage {
    #[serde(default = "default_true")]
    audio: bool,
    #[serde(default = "default_true")]
    video: bool,
    #[serde(default)]
    bitrate: u64,
}

fn default_true() -> bool {
    true
}

#[async_trait]
impl janus_plugin_api::JanusPlugin for EchoTestPlugin {
    fn name(&self) -> &'static str {
        "Janus EchoTest plugin"
    }

    fn package(&self) -> &'static str {
        "janus.plugin.echotest"
    }

    fn version(&self) -> u32 {
        1
    }

    fn version_string(&self) -> &'static str {
        "0.1.0"
    }

    fn description(&self) -> &'static str {
        "Echoes RTP audio/video back to the sender, with optional muting and bitrate cap."
    }

    fn author(&self) -> &'static str {
        "Steve Seguin"
    }

    async fn init(
        &mut self,
        callbacks: Arc<dyn PluginCallbacks>,
        _config_path: &Path,
    ) -> janus_plugin_api::Result<()> {
        info!("EchoTest plugin initialized");
        self.callbacks = Some(callbacks);
        Ok(())
    }

    async fn destroy(&mut self) -> janus_plugin_api::Result<()> {
        info!("EchoTest plugin destroyed");
        self.sessions.clear();
        self.callbacks = None;
        Ok(())
    }

    async fn create_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let key = (session.session_id, session.handle_id);
        self.sessions.insert(
            key,
            EchoSession {
                audio: true,
                video: true,
                bitrate: 0,
                active: false,
            },
        );
        debug!(session = %session, "echotest session created");
        Ok(())
    }

    async fn destroy_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let key = (session.session_id, session.handle_id);
        self.sessions.remove(&key);
        debug!(session = %session, "echotest session destroyed");
        Ok(())
    }

    fn query_session(
        &self,
        session: &PluginSession,
    ) -> janus_plugin_api::Result<serde_json::Value> {
        let key = (session.session_id, session.handle_id);
        if let Some(s) = self.sessions.get(&key) {
            Ok(json!({
                "audio": s.audio,
                "video": s.video,
                "bitrate": s.bitrate,
                "active": s.active,
            }))
        } else {
            Err(janus_plugin_api::Error::SessionNotFound(
                session.to_string(),
            ))
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

        // Parse the message body
        let msg: EchoTestMessage =
            serde_json::from_value(body.clone()).unwrap_or(EchoTestMessage {
                audio: true,
                video: true,
                bitrate: 0,
            });

        // Update session state
        if let Some(mut s) = self.sessions.get_mut(&key) {
            s.audio = msg.audio;
            s.video = msg.video;
            if msg.bitrate > 0 {
                s.bitrate = msg.bitrate;
            }
            debug!(
                session = %session,
                audio = msg.audio,
                video = msg.video,
                bitrate = msg.bitrate,
                "echotest config updated"
            );
        } else {
            return Err(janus_plugin_api::Error::SessionNotFound(
                session.to_string(),
            ));
        }

        // Build the response (matches C Janus echotest format)
        let result_body = json!({
            "echotest": "event",
            "result": "ok",
        });

        // If there's a JSEP offer, signal the core to handle WebRTC negotiation.
        // The core will create a PeerConnection, generate the SDP answer, and
        // push the event (with JSEP answer) back to the client.
        if let Some(ref offer) = jsep {
            if offer.jsep_type == JsepType::Offer {
                // Push the event asynchronously with JSEP answer indicator.
                // The core intercepts this and replaces with a real SDP answer.
                if let Some(ref callbacks) = self.callbacks {
                    let answer_jsep = Jsep {
                        jsep_type: JsepType::Answer,
                        sdp: offer.sdp.clone(), // placeholder — core replaces with real answer
                        trickle: offer.trickle,
                    };
                    let _ = callbacks
                        .push_event(session, transaction, result_body.clone(), Some(answer_jsep))
                        .await;
                }
                return Ok(PluginResult::OkWait {
                    hint: Some("Processing JSEP".into()),
                });
            }
        }

        Ok(PluginResult::Ok(PluginResultPayload {
            body: result_body,
            jsep: None,
        }))
    }

    fn setup_media(&self, session: &PluginSession) {
        let key = (session.session_id, session.handle_id);
        if let Some(mut s) = self.sessions.get_mut(&key) {
            s.active = true;
            debug!(session = %session, "echotest media ready");
        }
    }

    fn hangup_media(&self, session: &PluginSession, reason: &str) {
        let key = (session.session_id, session.handle_id);
        if let Some(mut s) = self.sessions.get_mut(&key) {
            s.active = false;
            debug!(session = %session, reason = reason, "echotest media hung up");
        }
    }

    fn incoming_rtp(&self, session: &PluginSession, packet: &RtpPacket) {
        let key = (session.session_id, session.handle_id);
        if let Some(s) = self.sessions.get(&key) {
            if !s.active {
                return;
            }
            // Check if this media type is enabled
            if (packet.video && !s.video) || (!packet.video && !s.audio) {
                return;
            }
            // Echo the packet back
            if let Some(ref callbacks) = self.callbacks {
                callbacks.relay_rtp(session, packet);
            }
        }
    }

    fn incoming_rtcp(&self, session: &PluginSession, packet: &RtcpPacket) {
        let key = (session.session_id, session.handle_id);
        if let Some(s) = self.sessions.get(&key) {
            if !s.active {
                return;
            }
            if let Some(ref callbacks) = self.callbacks {
                callbacks.relay_rtcp(session, packet);
            }
        }
    }

    fn slow_link(&self, session: &PluginSession, uplink: bool, lost: u32) {
        warn!(
            session = %session,
            uplink = uplink,
            lost_packets = lost,
            "slow link detected"
        );
    }
}

/// ABI version entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_echotest_abi_version() -> u32 {
    janus_plugin_api::PLUGIN_LOADER_ABI_VERSION
}

/// FFI entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_echotest_create() -> janus_plugin_api::PluginOpaqueHandle {
    janus_plugin_api::into_ffi_plugin(EchoTestPlugin::default())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use janus_plugin_api::JanusPlugin;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Mock plugin callbacks for testing.
    struct MockCallbacks {
        rtp_relayed: AtomicU64,
        rtcp_relayed: AtomicU64,
    }

    impl MockCallbacks {
        fn new() -> Self {
            Self {
                rtp_relayed: AtomicU64::new(0),
                rtcp_relayed: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl PluginCallbacks for MockCallbacks {
        fn relay_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {
            self.rtp_relayed.fetch_add(1, Ordering::Relaxed);
        }

        fn relay_rtcp(&self, _session: &PluginSession, _packet: &RtcpPacket) {
            self.rtcp_relayed.fetch_add(1, Ordering::Relaxed);
        }

        fn relay_data(&self, _session: &PluginSession, _label: &str, _data: &[u8]) {}

        async fn push_event(
            &self,
            _session: &PluginSession,
            _transaction: &str,
            _body: serde_json::Value,
            _jsep: Option<Jsep>,
        ) -> janus_plugin_api::Result<()> {
            Ok(())
        }

        fn close_pc(&self, _session: &PluginSession) {}
        fn end_session(&self, _session: &PluginSession) {}
        fn notify_event(&self, _plugin_name: &str, _event: serde_json::Value) {}
    }

    fn test_session() -> PluginSession {
        PluginSession::new(SessionId(1), HandleId(1))
    }

    fn make_rtp(video: bool) -> RtpPacket {
        RtpPacket::new(video, vec![0x80, 111, 0, 1, 0, 0, 0, 160, 0, 0, 3, 232])
    }

    fn make_rtcp() -> RtcpPacket {
        RtcpPacket::new(false, vec![0x80, 0xc9, 0, 1, 0, 0, 0, 1])
    }

    #[test]
    fn plugin_metadata() {
        let plugin = EchoTestPlugin::default();
        assert_eq!(plugin.name(), "Janus EchoTest plugin");
        assert_eq!(plugin.package(), "janus.plugin.echotest");
        assert_eq!(plugin.version(), 1);
        assert!(!plugin.description().is_empty());
    }

    #[tokio::test]
    async fn init_and_destroy() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();
        assert!(plugin.callbacks.is_some());
        plugin.destroy().await.unwrap();
        assert!(plugin.callbacks.is_none());
    }

    #[tokio::test]
    async fn create_and_destroy_session() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();
        assert_eq!(plugin.sessions.len(), 1);

        plugin.destroy_session(&session).await.unwrap();
        assert_eq!(plugin.sessions.len(), 0);
    }

    #[tokio::test]
    async fn query_session_returns_state() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();

        let info = plugin.query_session(&session).unwrap();
        assert_eq!(info["audio"], true);
        assert_eq!(info["video"], true);
        assert_eq!(info["bitrate"], 0);
        assert_eq!(info["active"], false);
    }

    #[tokio::test]
    async fn handle_message_updates_state() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "txn1",
                json!({"audio": false, "video": true, "bitrate": 128000}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["echotest"], "event");
                assert_eq!(payload.body["result"], "ok");
                assert!(payload.jsep.is_none());
            }
            _ => panic!("expected Ok result"),
        }

        // Verify session state was updated
        let info = plugin.query_session(&session).unwrap();
        assert_eq!(info["audio"], false);
        assert_eq!(info["bitrate"], 128000);
    }

    #[tokio::test]
    async fn handle_message_with_jsep_offer() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".into(),
            trickle: true,
        };

        let result = plugin
            .handle_message(&session, "txn2", json!({}), Some(offer))
            .await
            .unwrap();

        match result {
            PluginResult::OkWait { hint } => {
                // With a JSEP offer, the plugin delegates WebRTC negotiation
                // to the core and returns OkWait
                assert!(hint.is_some());
                assert!(hint.unwrap().contains("JSEP"));
            }
            _ => panic!("expected OkWait result, got {:?}", result),
        }
    }

    #[tokio::test]
    async fn rtp_echoed_when_active() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();
        plugin.setup_media(&session);

        // Send audio RTP
        plugin.incoming_rtp(&session, &make_rtp(false));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 1);

        // Send video RTP
        plugin.incoming_rtp(&session, &make_rtp(true));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn rtp_not_echoed_when_inactive() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();
        // Don't call setup_media — session should not be active

        plugin.incoming_rtp(&session, &make_rtp(false));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn audio_muted_suppresses_audio_rtp() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();
        plugin.setup_media(&session);

        // Mute audio
        plugin
            .handle_message(
                &session,
                "txn",
                json!({"audio": false, "video": true}),
                None,
            )
            .await
            .unwrap();

        // Audio should be suppressed
        plugin.incoming_rtp(&session, &make_rtp(false));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 0);

        // Video should still work
        plugin.incoming_rtp(&session, &make_rtp(true));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn video_muted_suppresses_video_rtp() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();
        plugin.setup_media(&session);

        // Mute video
        plugin
            .handle_message(
                &session,
                "txn",
                json!({"audio": true, "video": false}),
                None,
            )
            .await
            .unwrap();

        // Video should be suppressed
        plugin.incoming_rtp(&session, &make_rtp(true));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 0);

        // Audio should still work
        plugin.incoming_rtp(&session, &make_rtp(false));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn rtcp_echoed_when_active() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();
        plugin.setup_media(&session);

        plugin.incoming_rtcp(&session, &make_rtcp());
        assert_eq!(cb_clone.rtcp_relayed.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn hangup_stops_echoing() {
        let mut plugin = EchoTestPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session();
        plugin.create_session(&session).await.unwrap();
        plugin.setup_media(&session);

        // Verify active
        plugin.incoming_rtp(&session, &make_rtp(false));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 1);

        // Hangup
        plugin.hangup_media(&session, "user hangup");

        // Should no longer echo
        plugin.incoming_rtp(&session, &make_rtp(false));
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 1); // still 1
    }

    #[test]
    fn ffi_create_returns_valid_plugin() {
        let raw = janus_plugin_echotest_create();
        assert!(!raw.is_null());
        // Clean up
        unsafe {
            let _ = janus_plugin_api::from_ffi_plugin(raw);
        }
    }
}
