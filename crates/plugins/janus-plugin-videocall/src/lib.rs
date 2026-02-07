//! Janus VideoCall plugin — peer-to-peer video calls.
//!
//! Implements a simple 1-to-1 video call model where users register usernames,
//! then call each other. Media is relayed through the server.

use async_trait::async_trait;
use dashmap::DashMap;
use janus_plugin_api::{
    HandleId, Jsep, JsepType, PluginCallbacks, PluginResult, PluginResultPayload, PluginSession,
    RtcpPacket, RtpPacket, SessionId,
};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

// Error codes matching C Janus videocall plugin
const VIDEOCALL_ERROR_MISSING_ELEMENT: u32 = 475;
const VIDEOCALL_ERROR_INVALID_REQUEST: u32 = 474;
const VIDEOCALL_ERROR_REGISTER_FIRST: u32 = 473;
const VIDEOCALL_ERROR_USERNAME_TAKEN: u32 = 476;
const VIDEOCALL_ERROR_ALREADY_REGISTERED: u32 = 477;
const VIDEOCALL_ERROR_NO_SUCH_USERNAME: u32 = 478;
const VIDEOCALL_ERROR_ALREADY_IN_CALL: u32 = 480;
const VIDEOCALL_ERROR_NO_CALL: u32 = 481;
const VIDEOCALL_ERROR_MISSING_SDP: u32 = 482;

type SessionKey = (SessionId, HandleId);

/// Per-session state for a VideoCall participant.
struct VideoCallSession {
    /// Registered username (None if not yet registered).
    username: Option<String>,
    /// Linked peer's session key (set when in a call).
    peer: Option<SessionKey>,
    /// Whether audio relaying is active.
    audio_active: bool,
    /// Whether video relaying is active.
    video_active: bool,
    /// Bitrate cap in bps (0 = no cap).
    bitrate: u64,
}

/// The VideoCall plugin.
pub struct VideoCallPlugin {
    callbacks: Option<Arc<dyn PluginCallbacks>>,
    /// Per-handle session state.
    sessions: DashMap<SessionKey, VideoCallSession>,
    /// Username → session key mapping for lookups.
    usernames: DashMap<String, SessionKey>,
}

impl Default for VideoCallPlugin {
    fn default() -> Self {
        Self {
            callbacks: None,
            sessions: DashMap::new(),
            usernames: DashMap::new(),
        }
    }
}

impl VideoCallPlugin {
    fn callbacks(&self) -> &Arc<dyn PluginCallbacks> {
        self.callbacks.as_ref().expect("plugin not initialized")
    }

    fn error_result(code: u32, reason: &str) -> PluginResult {
        PluginResult::Ok(PluginResultPayload {
            body: json!({
                "videocall": "event",
                "error_code": code,
                "error": reason
            }),
            jsep: None,
        })
    }

    fn handle_register(&self, key: SessionKey, body: &serde_json::Value) -> PluginResult {
        let username = match body["username"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => {
                return Self::error_result(
                    VIDEOCALL_ERROR_MISSING_ELEMENT,
                    "Missing mandatory element (username)",
                );
            }
        };

        // Check if this session is already registered
        if let Some(session) = self.sessions.get(&key) {
            if session.username.is_some() {
                return Self::error_result(
                    VIDEOCALL_ERROR_ALREADY_REGISTERED,
                    "Already registered a username",
                );
            }
        }

        // Check if the username is taken
        if self.usernames.contains_key(&username) {
            return Self::error_result(
                VIDEOCALL_ERROR_USERNAME_TAKEN,
                &format!("Username '{username}' already taken"),
            );
        }

        // Register the username
        if let Some(mut session) = self.sessions.get_mut(&key) {
            session.username = Some(username.clone());
            self.usernames.insert(username.clone(), key);
        }

        PluginResult::Ok(PluginResultPayload {
            body: json!({
                "videocall": "event",
                "result": {
                    "event": "registered",
                    "username": username
                }
            }),
            jsep: None,
        })
    }

    fn handle_list(&self) -> PluginResult {
        let list: Vec<String> = self.usernames.iter().map(|r| r.key().clone()).collect();
        PluginResult::Ok(PluginResultPayload {
            body: json!({
                "videocall": "event",
                "result": {
                    "list": list
                }
            }),
            jsep: None,
        })
    }

    async fn handle_call(
        &self,
        key: SessionKey,
        body: &serde_json::Value,
        jsep: Option<Jsep>,
    ) -> PluginResult {
        // Check caller is registered
        let caller_username = {
            let session = match self.sessions.get(&key) {
                Some(s) => s,
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_REGISTER_FIRST,
                        "Register a username first",
                    );
                }
            };
            match &session.username {
                Some(u) => u.clone(),
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_REGISTER_FIRST,
                        "Register a username first",
                    );
                }
            }
        };

        // Check caller is not already in a call
        if let Some(session) = self.sessions.get(&key) {
            if session.peer.is_some() {
                return Self::error_result(VIDEOCALL_ERROR_ALREADY_IN_CALL, "Already in a call");
            }
        }

        // Validate JSEP offer
        let offer_jsep = match jsep {
            Some(j) if j.jsep_type == JsepType::Offer => j,
            _ => {
                return Self::error_result(VIDEOCALL_ERROR_MISSING_SDP, "Missing SDP offer");
            }
        };

        // Look up the peer
        let peer_username = match body["username"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => {
                return Self::error_result(
                    VIDEOCALL_ERROR_MISSING_ELEMENT,
                    "Missing mandatory element (username)",
                );
            }
        };

        let peer_key = match self.usernames.get(&peer_username) {
            Some(pk) => *pk.value(),
            None => {
                return Self::error_result(
                    VIDEOCALL_ERROR_NO_SUCH_USERNAME,
                    &format!("No such user '{peer_username}'"),
                );
            }
        };

        // Check peer is not already in a call
        if let Some(peer_session) = self.sessions.get(&peer_key) {
            if peer_session.peer.is_some() {
                return Self::error_result(
                    VIDEOCALL_ERROR_ALREADY_IN_CALL,
                    &format!("User '{peer_username}' busy"),
                );
            }
        }

        // Link the two peers
        if let Some(mut session) = self.sessions.get_mut(&key) {
            session.peer = Some(peer_key);
        }
        if let Some(mut peer_session) = self.sessions.get_mut(&peer_key) {
            peer_session.peer = Some(key);
        }

        // Push incoming call event to the peer
        let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
        let _ = self
            .callbacks()
            .push_event(
                &peer_ps,
                "",
                json!({
                    "videocall": "event",
                    "result": {
                        "event": "incomingcall",
                        "username": caller_username
                    }
                }),
                Some(offer_jsep),
            )
            .await;

        PluginResult::Ok(PluginResultPayload {
            body: json!({
                "videocall": "event",
                "result": {
                    "event": "calling"
                }
            }),
            jsep: None,
        })
    }

    async fn handle_accept(&self, key: SessionKey, jsep: Option<Jsep>) -> PluginResult {
        // Check registered
        let username = {
            let session = match self.sessions.get(&key) {
                Some(s) => s,
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_REGISTER_FIRST,
                        "Register a username first",
                    );
                }
            };
            match &session.username {
                Some(u) => u.clone(),
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_REGISTER_FIRST,
                        "Register a username first",
                    );
                }
            }
        };

        // Check in a call
        let peer_key = {
            let session = self.sessions.get(&key).unwrap();
            match session.peer {
                Some(pk) => pk,
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_NO_CALL,
                        "No incoming call to accept",
                    );
                }
            }
        };

        // Validate JSEP answer
        let answer_jsep = match jsep {
            Some(j) if j.jsep_type == JsepType::Answer => j,
            _ => {
                return Self::error_result(VIDEOCALL_ERROR_MISSING_SDP, "Missing SDP answer");
            }
        };

        // Push accepted event to the caller (peer)
        let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
        let _ = self
            .callbacks()
            .push_event(
                &peer_ps,
                "",
                json!({
                    "videocall": "event",
                    "result": {
                        "event": "accepted",
                        "username": username
                    }
                }),
                Some(answer_jsep),
            )
            .await;

        PluginResult::Ok(PluginResultPayload {
            body: json!({
                "videocall": "event",
                "result": {
                    "event": "accepted",
                    "username": username
                }
            }),
            jsep: None,
        })
    }

    async fn handle_set(
        &self,
        key: SessionKey,
        body: &serde_json::Value,
        jsep: Option<Jsep>,
    ) -> PluginResult {
        // Update session settings
        if let Some(mut session) = self.sessions.get_mut(&key) {
            if let Some(audio) = body["audio"].as_bool() {
                session.audio_active = audio;
            }
            if let Some(video) = body["video"].as_bool() {
                session.video_active = video;
            }
            if let Some(bitrate) = body["bitrate"].as_u64() {
                session.bitrate = bitrate;
            }
        }

        // If JSEP present and in a call, forward renegotiation to peer
        if let Some(jsep_val) = jsep {
            if let Some(session) = self.sessions.get(&key) {
                if let Some(peer_key) = session.peer {
                    let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
                    let _ = self
                        .callbacks()
                        .push_event(
                            &peer_ps,
                            "",
                            json!({
                                "videocall": "event",
                                "result": {
                                    "event": "update"
                                }
                            }),
                            Some(jsep_val),
                        )
                        .await;
                }
            }
        }

        PluginResult::Ok(PluginResultPayload {
            body: json!({
                "videocall": "event",
                "result": {
                    "event": "set"
                }
            }),
            jsep: None,
        })
    }

    fn handle_hangup_request(&self, key: SessionKey) -> PluginResult {
        // Check registered
        let username = {
            let session = match self.sessions.get(&key) {
                Some(s) => s,
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_REGISTER_FIRST,
                        "Register a username first",
                    );
                }
            };
            match &session.username {
                Some(u) => u.clone(),
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_REGISTER_FIRST,
                        "Register a username first",
                    );
                }
            }
        };

        // Check in a call
        let peer_key = {
            let session = self.sessions.get(&key).unwrap();
            match session.peer {
                Some(pk) => pk,
                None => {
                    return Self::error_result(
                        VIDEOCALL_ERROR_NO_CALL,
                        "No active call to hang up",
                    );
                }
            }
        };

        // Clear peer linkage on both sides
        if let Some(mut session) = self.sessions.get_mut(&key) {
            session.peer = None;
        }
        if let Some(mut peer_session) = self.sessions.get_mut(&peer_key) {
            peer_session.peer = None;
        }

        // Notify peer (done synchronously via callbacks; push_event is async
        // but we need to return synchronously here — schedule it)
        let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
        let callbacks = self.callbacks().clone();
        let hangup_username = username.clone();
        tokio::spawn(async move {
            let _ = callbacks
                .push_event(
                    &peer_ps,
                    "",
                    json!({
                        "videocall": "event",
                        "result": {
                            "event": "hangup",
                            "username": hangup_username,
                            "reason": "We did the location location"
                        }
                    }),
                    None,
                )
                .await;
        });

        PluginResult::Ok(PluginResultPayload {
            body: json!({
                "videocall": "event",
                "result": {
                    "event": "hangup",
                    "username": username,
                    "reason": "We did the location location"
                }
            }),
            jsep: None,
        })
    }
}

#[async_trait]
impl janus_plugin_api::JanusPlugin for VideoCallPlugin {
    fn name(&self) -> &'static str {
        "Janus VideoCall plugin"
    }

    fn package(&self) -> &'static str {
        "janus.plugin.videocall"
    }

    fn version(&self) -> u32 {
        1
    }

    fn version_string(&self) -> &'static str {
        "0.1.0"
    }

    fn description(&self) -> &'static str {
        "Peer-to-peer video calls between WebRTC users."
    }

    fn author(&self) -> &'static str {
        "Janus Rust Contributors"
    }

    async fn init(
        &mut self,
        callbacks: Arc<dyn PluginCallbacks>,
        _config_path: &Path,
    ) -> janus_plugin_api::Result<()> {
        info!("VideoCall plugin initialized");
        self.callbacks = Some(callbacks);
        Ok(())
    }

    async fn destroy(&mut self) -> janus_plugin_api::Result<()> {
        info!("VideoCall plugin destroyed");
        self.sessions.clear();
        self.usernames.clear();
        self.callbacks = None;
        Ok(())
    }

    async fn create_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let key = (session.session_id, session.handle_id);
        debug!(?key, "VideoCall: session created");
        self.sessions.insert(
            key,
            VideoCallSession {
                username: None,
                peer: None,
                audio_active: true,
                video_active: true,
                bitrate: 0,
            },
        );
        Ok(())
    }

    async fn destroy_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let key = (session.session_id, session.handle_id);
        debug!(?key, "VideoCall: session destroyed");

        // If in a call, notify peer
        if let Some((_, s)) = self.sessions.remove(&key) {
            // Remove username mapping
            if let Some(ref username) = s.username {
                self.usernames.remove(username);
            }
            // Notify peer of hangup
            if let Some(peer_key) = s.peer {
                if let Some(mut peer_session) = self.sessions.get_mut(&peer_key) {
                    peer_session.peer = None;
                }
                let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
                let caller_name = s.username.unwrap_or_default();
                let _ = self
                    .callbacks()
                    .push_event(
                        &peer_ps,
                        "",
                        json!({
                            "videocall": "event",
                            "result": {
                                "event": "hangup",
                                "username": caller_name,
                                "reason": "User disconnected"
                            }
                        }),
                        None,
                    )
                    .await;
            }
        }
        Ok(())
    }

    fn query_session(
        &self,
        session: &PluginSession,
    ) -> janus_plugin_api::Result<serde_json::Value> {
        let key = (session.session_id, session.handle_id);
        match self.sessions.get(&key) {
            Some(s) => Ok(json!({
                "username": s.username,
                "in_call": s.peer.is_some(),
                "audio_active": s.audio_active,
                "video_active": s.video_active,
                "bitrate": s.bitrate,
            })),
            None => Ok(json!({"error": "session not found"})),
        }
    }

    async fn handle_message(
        &self,
        session: &PluginSession,
        _transaction: &str,
        body: serde_json::Value,
        jsep: Option<Jsep>,
    ) -> janus_plugin_api::Result<PluginResult> {
        let key = (session.session_id, session.handle_id);
        let request = body["request"].as_str().unwrap_or("");

        let result = match request {
            "register" => self.handle_register(key, &body),
            "list" => self.handle_list(),
            "call" => self.handle_call(key, &body, jsep).await,
            "accept" => self.handle_accept(key, jsep).await,
            "set" => self.handle_set(key, &body, jsep).await,
            "hangup" => self.handle_hangup_request(key),
            _ => Self::error_result(
                VIDEOCALL_ERROR_INVALID_REQUEST,
                &format!("Unknown request '{request}'"),
            ),
        };

        Ok(result)
    }

    fn setup_media(&self, session: &PluginSession) {
        debug!(session = %session, "VideoCall: media setup");
    }

    fn hangup_media(&self, session: &PluginSession, reason: &str) {
        let key = (session.session_id, session.handle_id);
        debug!(?key, reason = reason, "VideoCall: media hangup");

        // Clear peer linkage
        if let Some(mut s) = self.sessions.get_mut(&key) {
            if let Some(peer_key) = s.peer.take() {
                if let Some(mut peer_session) = self.sessions.get_mut(&peer_key) {
                    peer_session.peer = None;
                }
                // Notify peer
                let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
                let username = s.username.clone().unwrap_or_default();
                let hangup_reason = reason.to_string();
                let callbacks = self.callbacks().clone();
                tokio::spawn(async move {
                    let _ = callbacks
                        .push_event(
                            &peer_ps,
                            "",
                            json!({
                                "videocall": "event",
                                "result": {
                                    "event": "hangup",
                                    "username": username,
                                    "reason": hangup_reason
                                }
                            }),
                            None,
                        )
                        .await;
                });
            }
        }
    }

    fn incoming_rtp(&self, session: &PluginSession, packet: &RtpPacket) {
        let key = (session.session_id, session.handle_id);
        if let Some(s) = self.sessions.get(&key) {
            // Check if media type is enabled
            let enabled = if packet.video {
                s.video_active
            } else {
                s.audio_active
            };
            if enabled {
                if let Some(peer_key) = s.peer {
                    let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
                    self.callbacks().relay_rtp(&peer_ps, packet);
                }
            }
        }
    }

    fn incoming_rtcp(&self, session: &PluginSession, packet: &RtcpPacket) {
        let key = (session.session_id, session.handle_id);
        if let Some(s) = self.sessions.get(&key) {
            if let Some(peer_key) = s.peer {
                let peer_ps = PluginSession::new(peer_key.0, peer_key.1);
                self.callbacks().relay_rtcp(&peer_ps, packet);
            }
        }
    }
}

/// ABI version entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_videocall_abi_version() -> u32 {
    janus_plugin_api::PLUGIN_LOADER_ABI_VERSION
}

/// FFI entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_videocall_create() -> janus_plugin_api::PluginOpaqueHandle {
    janus_plugin_api::into_ffi_plugin(VideoCallPlugin::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use janus_plugin_api::JanusPlugin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    struct MockCallbacks {
        events: Mutex<Vec<(PluginSession, serde_json::Value, Option<Jsep>)>>,
        rtp_count: AtomicU64,
        rtcp_count: AtomicU64,
    }

    impl MockCallbacks {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                rtp_count: AtomicU64::new(0),
                rtcp_count: AtomicU64::new(0),
            }
        }

        fn events(&self) -> Vec<(PluginSession, serde_json::Value, Option<Jsep>)> {
            self.events.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl PluginCallbacks for MockCallbacks {
        fn relay_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {
            self.rtp_count.fetch_add(1, Ordering::Relaxed);
        }

        fn relay_rtcp(&self, _session: &PluginSession, _packet: &RtcpPacket) {
            self.rtcp_count.fetch_add(1, Ordering::Relaxed);
        }

        fn relay_data(&self, _session: &PluginSession, _label: &str, _data: &[u8]) {}

        async fn push_event(
            &self,
            session: &PluginSession,
            _transaction: &str,
            body: serde_json::Value,
            jsep: Option<Jsep>,
        ) -> janus_plugin_api::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push((session.clone(), body, jsep));
            Ok(())
        }

        fn close_pc(&self, _session: &PluginSession) {}
        fn end_session(&self, _session: &PluginSession) {}
        fn notify_event(&self, _plugin_name: &str, _event: serde_json::Value) {}
    }

    async fn setup() -> (VideoCallPlugin, Arc<MockCallbacks>) {
        let callbacks = Arc::new(MockCallbacks::new());
        let mut plugin = VideoCallPlugin::default();
        plugin
            .init(
                callbacks.clone() as Arc<dyn PluginCallbacks>,
                Path::new("/tmp"),
            )
            .await
            .unwrap();
        (plugin, callbacks)
    }

    fn make_session(sid: u64, hid: u64) -> PluginSession {
        PluginSession::new(SessionId(sid), HandleId(hid))
    }

    #[test]
    fn plugin_metadata() {
        let plugin = VideoCallPlugin::default();
        assert_eq!(plugin.name(), "Janus VideoCall plugin");
        assert_eq!(plugin.package(), "janus.plugin.videocall");
        assert_eq!(plugin.version(), 1);
        assert_eq!(plugin.version_string(), "0.1.0");
        assert_eq!(plugin.author(), "Janus Rust Contributors");
        assert!(!plugin.description().is_empty());
    }

    #[test]
    fn ffi_create_returns_valid_plugin() {
        let raw = janus_plugin_videocall_create();
        assert!(!raw.is_null());
        unsafe {
            let _ = janus_plugin_api::from_ffi_plugin(raw);
        }
    }

    #[tokio::test]
    async fn register_username() {
        let (plugin, _callbacks) = setup().await;
        let session = make_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["result"]["event"], "registered");
                assert_eq!(payload.body["result"]["username"], "alice");
            }
            _ => panic!("expected Ok result"),
        }

        // Verify query_session shows the username
        let info = plugin.query_session(&session).unwrap();
        assert_eq!(info["username"], "alice");
    }

    #[tokio::test]
    async fn register_duplicate_errors() {
        let (plugin, _callbacks) = setup().await;
        let s1 = make_session(1, 1);
        let s2 = make_session(2, 2);
        plugin.create_session(&s1).await.unwrap();
        plugin.create_session(&s2).await.unwrap();

        plugin
            .handle_message(
                &s1,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(
                &s2,
                "t2",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["error_code"], VIDEOCALL_ERROR_USERNAME_TAKEN);
            }
            _ => panic!("expected Ok result with error"),
        }
    }

    #[tokio::test]
    async fn register_already_registered_errors() {
        let (plugin, _callbacks) = setup().await;
        let session = make_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(
                    payload.body["error_code"],
                    VIDEOCALL_ERROR_ALREADY_REGISTERED
                );
            }
            _ => panic!("expected Ok result with error"),
        }
    }

    #[tokio::test]
    async fn list_usernames() {
        let (plugin, _callbacks) = setup().await;
        let s1 = make_session(1, 1);
        let s2 = make_session(2, 2);
        let s3 = make_session(3, 3);
        plugin.create_session(&s1).await.unwrap();
        plugin.create_session(&s2).await.unwrap();
        plugin.create_session(&s3).await.unwrap();

        plugin
            .handle_message(
                &s1,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &s2,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &s3,
                "t3",
                json!({"request": "register", "username": "charlie"}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(&s1, "t4", json!({"request": "list"}), None)
            .await
            .unwrap();
        match result {
            PluginResult::Ok(payload) => {
                let list = payload.body["result"]["list"].as_array().unwrap();
                assert_eq!(list.len(), 3);
                let names: Vec<String> = list
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
                assert!(names.contains(&"alice".to_string()));
                assert!(names.contains(&"bob".to_string()));
                assert!(names.contains(&"charlie".to_string()));
            }
            _ => panic!("expected Ok result"),
        }
    }

    #[tokio::test]
    async fn call_lifecycle() {
        let (plugin, callbacks) = setup().await;
        let alice = make_session(1, 1);
        let bob = make_session(2, 2);
        plugin.create_session(&alice).await.unwrap();
        plugin.create_session(&bob).await.unwrap();

        plugin
            .handle_message(
                &alice,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &bob,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();

        // Alice calls Bob
        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n".to_string(),
            trickle: false,
        };
        let result = plugin
            .handle_message(
                &alice,
                "t3",
                json!({"request": "call", "username": "bob"}),
                Some(offer),
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["result"]["event"], "calling");
            }
            _ => panic!("expected calling result"),
        }

        // Check Bob received incomingcall event
        let events = callbacks.events();
        assert!(!events.is_empty());
        let (target, body, jsep) = &events[0];
        assert_eq!(target.session_id, SessionId(2));
        assert_eq!(target.handle_id, HandleId(2));
        assert_eq!(body["result"]["event"], "incomingcall");
        assert_eq!(body["result"]["username"], "alice");
        assert!(jsep.is_some());

        // Bob accepts
        let answer = Jsep {
            jsep_type: JsepType::Answer,
            sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n".to_string(),
            trickle: false,
        };
        let result = plugin
            .handle_message(&bob, "t4", json!({"request": "accept"}), Some(answer))
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["result"]["event"], "accepted");
            }
            _ => panic!("expected accepted result"),
        }

        // Check Alice received accepted event
        let events = callbacks.events();
        assert!(events.len() >= 2);
        let (target, body, _) = &events[1];
        assert_eq!(target.session_id, SessionId(1));
        assert_eq!(body["result"]["event"], "accepted");
    }

    #[tokio::test]
    async fn call_without_register_errors() {
        let (plugin, _callbacks) = setup().await;
        let session = make_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".to_string(),
            trickle: false,
        };
        let result = plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "call", "username": "bob"}),
                Some(offer),
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["error_code"], VIDEOCALL_ERROR_REGISTER_FIRST);
            }
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn call_nonexistent_user_errors() {
        let (plugin, _callbacks) = setup().await;
        let session = make_session(1, 1);
        plugin.create_session(&session).await.unwrap();
        plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".to_string(),
            trickle: false,
        };
        let result = plugin
            .handle_message(
                &session,
                "t2",
                json!({"request": "call", "username": "nobody"}),
                Some(offer),
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["error_code"], VIDEOCALL_ERROR_NO_SUCH_USERNAME);
            }
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn call_already_in_call_errors() {
        let (plugin, _callbacks) = setup().await;
        let alice = make_session(1, 1);
        let bob = make_session(2, 2);
        let charlie = make_session(3, 3);
        plugin.create_session(&alice).await.unwrap();
        plugin.create_session(&bob).await.unwrap();
        plugin.create_session(&charlie).await.unwrap();

        plugin
            .handle_message(
                &alice,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &bob,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &charlie,
                "t3",
                json!({"request": "register", "username": "charlie"}),
                None,
            )
            .await
            .unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".to_string(),
            trickle: false,
        };
        // Alice calls Bob
        plugin
            .handle_message(
                &alice,
                "t4",
                json!({"request": "call", "username": "bob"}),
                Some(offer.clone()),
            )
            .await
            .unwrap();

        // Alice tries to call Charlie — should fail
        let result = plugin
            .handle_message(
                &alice,
                "t5",
                json!({"request": "call", "username": "charlie"}),
                Some(offer),
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["error_code"], VIDEOCALL_ERROR_ALREADY_IN_CALL);
            }
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn hangup_clears_peer() {
        let (plugin, _callbacks) = setup().await;
        let alice = make_session(1, 1);
        let bob = make_session(2, 2);
        plugin.create_session(&alice).await.unwrap();
        plugin.create_session(&bob).await.unwrap();

        plugin
            .handle_message(
                &alice,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &bob,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".to_string(),
            trickle: false,
        };
        plugin
            .handle_message(
                &alice,
                "t3",
                json!({"request": "call", "username": "bob"}),
                Some(offer),
            )
            .await
            .unwrap();

        // Both should be in call
        let alice_info = plugin.query_session(&alice).unwrap();
        assert_eq!(alice_info["in_call"], true);
        let bob_info = plugin.query_session(&bob).unwrap();
        assert_eq!(bob_info["in_call"], true);

        // Alice hangs up
        let result = plugin
            .handle_message(&alice, "t4", json!({"request": "hangup"}), None)
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["result"]["event"], "hangup");
            }
            _ => panic!("expected hangup result"),
        }

        // Wait for the spawned push_event task
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Both should be out of call
        let alice_info = plugin.query_session(&alice).unwrap();
        assert_eq!(alice_info["in_call"], false);
        let bob_info = plugin.query_session(&bob).unwrap();
        assert_eq!(bob_info["in_call"], false);
    }

    #[tokio::test]
    async fn set_audio_video_toggle() {
        let (plugin, callbacks) = setup().await;
        let alice = make_session(1, 1);
        let bob = make_session(2, 2);
        plugin.create_session(&alice).await.unwrap();
        plugin.create_session(&bob).await.unwrap();

        plugin
            .handle_message(
                &alice,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &bob,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".to_string(),
            trickle: false,
        };
        plugin
            .handle_message(
                &alice,
                "t3",
                json!({"request": "call", "username": "bob"}),
                Some(offer),
            )
            .await
            .unwrap();

        // Disable audio
        let result = plugin
            .handle_message(
                &alice,
                "t4",
                json!({"request": "set", "audio": false}),
                None,
            )
            .await
            .unwrap();
        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["result"]["event"], "set");
            }
            _ => panic!("expected set result"),
        }

        // Verify audio is disabled
        let info = plugin.query_session(&alice).unwrap();
        assert_eq!(info["audio_active"], false);
        assert_eq!(info["video_active"], true);

        // Verify RTP filtering: audio should NOT be relayed
        let audio_packet = RtpPacket::new(false, vec![0x80, 111, 0, 1]);
        plugin.incoming_rtp(&alice, &audio_packet);
        assert_eq!(callbacks.rtp_count.load(Ordering::Relaxed), 0);

        // Video SHOULD be relayed
        let video_packet = RtpPacket::new(true, vec![0x80, 96, 0, 1]);
        plugin.incoming_rtp(&alice, &video_packet);
        assert_eq!(callbacks.rtp_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn rtp_relay_between_peers() {
        let (plugin, callbacks) = setup().await;
        let alice = make_session(1, 1);
        let bob = make_session(2, 2);
        plugin.create_session(&alice).await.unwrap();
        plugin.create_session(&bob).await.unwrap();

        plugin
            .handle_message(
                &alice,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &bob,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".to_string(),
            trickle: false,
        };
        plugin
            .handle_message(
                &alice,
                "t3",
                json!({"request": "call", "username": "bob"}),
                Some(offer),
            )
            .await
            .unwrap();

        // Alice sends RTP — should be relayed to Bob
        let packet = RtpPacket::new(true, vec![0x80, 96, 0, 1]);
        plugin.incoming_rtp(&alice, &packet);
        assert_eq!(callbacks.rtp_count.load(Ordering::Relaxed), 1);

        // Bob sends RTCP — should be relayed to Alice
        let rtcp = RtcpPacket::new(true, vec![0x80, 0xc9, 0, 1]);
        plugin.incoming_rtcp(&bob, &rtcp);
        assert_eq!(callbacks.rtcp_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn destroy_session_cleans_up() {
        let (plugin, callbacks) = setup().await;
        let alice = make_session(1, 1);
        let bob = make_session(2, 2);
        plugin.create_session(&alice).await.unwrap();
        plugin.create_session(&bob).await.unwrap();

        plugin
            .handle_message(
                &alice,
                "t1",
                json!({"request": "register", "username": "alice"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(
                &bob,
                "t2",
                json!({"request": "register", "username": "bob"}),
                None,
            )
            .await
            .unwrap();

        let offer = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".to_string(),
            trickle: false,
        };
        plugin
            .handle_message(
                &alice,
                "t3",
                json!({"request": "call", "username": "bob"}),
                Some(offer),
            )
            .await
            .unwrap();

        // Destroy Alice's session mid-call
        plugin.destroy_session(&alice).await.unwrap();

        // Alice's username should be freed
        assert!(!plugin.usernames.contains_key("alice"));

        // Bob should be notified (hangup event)
        let events = callbacks.events();
        let hangup_events: Vec<_> = events
            .iter()
            .filter(|(_, body, _)| body["result"]["event"] == "hangup")
            .collect();
        assert!(!hangup_events.is_empty());

        // Bob should no longer be in a call
        let bob_info = plugin.query_session(&bob).unwrap();
        assert_eq!(bob_info["in_call"], false);
    }

    #[tokio::test]
    async fn unknown_request_returns_error() {
        let (plugin, _callbacks) = setup().await;
        let session = make_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(&session, "t1", json!({"request": "nonexistent"}), None)
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["error_code"], VIDEOCALL_ERROR_INVALID_REQUEST);
            }
            _ => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn register_missing_username_errors() {
        let (plugin, _callbacks) = setup().await;
        let session = make_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(&session, "t1", json!({"request": "register"}), None)
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["error_code"], VIDEOCALL_ERROR_MISSING_ELEMENT);
            }
            _ => panic!("expected error"),
        }
    }
}
