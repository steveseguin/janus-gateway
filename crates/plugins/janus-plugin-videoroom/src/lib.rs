//! Janus VideoRoom plugin — SFU-based multi-party video conferencing.
//!
//! Implements the Janus VideoRoom wire protocol for compatibility with janus.js.
//! Publishers send media which is fanned out to all subscribers.

pub mod room;

use async_trait::async_trait;
use dashmap::DashMap;
use janus_plugin_api::{
    HandleId, Jsep, JsepType, PluginCallbacks, PluginResult, PluginResultPayload, PluginSession,
    RtcpPacket, RtpPacket, SessionId,
};
use room::{PublisherInfo, RoomConfig, RoomId, RoomRegistry, SubscriberInfo};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Global counter for auto-generating user IDs.
static NEXT_USER_ID: AtomicU64 = AtomicU64::new(1);

/// Per-handle state (each browser handle gets one).
#[derive(Debug)]
enum HandleState {
    /// Not yet joined any room.
    None,
    /// Joined as publisher.
    Publisher {
        room_id: RoomId,
        user_id: u64,
        display: Option<String>,
        active: bool,
    },
    /// Joined as subscriber.
    Subscriber {
        room_id: RoomId,
        user_id: u64,
        feed: u64,
        active: bool,
    },
}

/// The VideoRoom plugin.
pub struct VideoRoomPlugin {
    callbacks: Option<Arc<dyn PluginCallbacks>>,
    rooms: RoomRegistry,
    /// Maps (session, handle) to their state.
    handles: DashMap<(SessionId, HandleId), HandleState>,
    /// Maps publisher user_id to their (session, handle) for media forwarding.
    publisher_handles: DashMap<u64, PluginSession>,
}

impl Default for VideoRoomPlugin {
    fn default() -> Self {
        Self {
            callbacks: None,
            rooms: RoomRegistry::new(),
            handles: DashMap::new(),
            publisher_handles: DashMap::new(),
        }
    }
}

/// Incoming message request type.
#[derive(Debug, Deserialize)]
struct VideoRoomRequest {
    request: String,
    #[serde(default)]
    room: Option<RoomId>,
    #[serde(default)]
    ptype: Option<String>,
    #[serde(default)]
    feed: Option<u64>,
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    max_publishers: Option<u32>,
    #[serde(default)]
    bitrate: Option<u64>,
    #[serde(default)]
    is_private: Option<bool>,
    #[serde(default)]
    pin: Option<String>,
    #[serde(default)]
    secret: Option<String>,
    // Audio/video toggles for configure
    #[serde(default = "default_true")]
    audio: bool,
    #[serde(default = "default_true")]
    video: bool,
}

fn default_true() -> bool {
    true
}

impl VideoRoomPlugin {
    /// Handle a "join" request as publisher.
    fn handle_join_publisher(
        &self,
        session: &PluginSession,
        room_id: RoomId,
        display: Option<String>,
    ) -> Result<serde_json::Value, String> {
        let room = self
            .rooms
            .get(room_id)
            .ok_or_else(|| format!("No such room {room_id}"))?;

        // Check max publishers
        if room.publishers.len() as u32 >= room.config.max_publishers {
            return Err("Room is full".into());
        }

        let user_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);
        let key = (session.session_id, session.handle_id);

        // Add to room
        room.publishers.insert(
            user_id,
            PublisherInfo {
                user_id,
                display: display.clone(),
                audio_codec: Some("opus".into()),
                video_codec: Some("vp8".into()),
                talking: false,
            },
        );

        // Track handle state
        self.handles.insert(
            key,
            HandleState::Publisher {
                room_id,
                user_id,
                display: display.clone(),
                active: false,
            },
        );
        self.publisher_handles.insert(user_id, session.clone());

        // Build publisher list (other publishers in the room)
        let publishers: Vec<serde_json::Value> = room
            .publishers
            .iter()
            .filter(|p| *p.key() != user_id)
            .map(|p| {
                json!({
                    "id": p.user_id,
                    "display": p.display,
                    "audio_codec": p.audio_codec,
                    "video_codec": p.video_codec,
                    "talking": p.talking,
                })
            })
            .collect();

        // Notify other publishers about this new publisher
        self.notify_publishers_in_room(room_id, user_id, &display);

        debug!(user_id = user_id, room = room_id, "publisher joined");

        Ok(json!({
            "videoroom": "joined",
            "room": room_id,
            "id": user_id,
            "description": room.config.description,
            "publishers": publishers,
        }))
    }

    /// Handle a "join" request as subscriber.
    fn handle_join_subscriber(
        &self,
        session: &PluginSession,
        room_id: RoomId,
        feed: u64,
    ) -> Result<serde_json::Value, String> {
        let room = self
            .rooms
            .get(room_id)
            .ok_or_else(|| format!("No such room {room_id}"))?;

        // Verify the feed exists
        if !room.publishers.contains_key(&feed) {
            return Err(format!("No such feed {feed} in room {room_id}"));
        }

        let user_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);
        let key = (session.session_id, session.handle_id);

        // Add subscriber to room
        room.subscribers.insert(
            user_id,
            SubscriberInfo {
                user_id,
                feed,
                paused: false,
            },
        );

        // Track handle state
        self.handles.insert(
            key,
            HandleState::Subscriber {
                room_id,
                user_id,
                feed,
                active: false,
            },
        );

        debug!(
            user_id = user_id,
            room = room_id,
            feed = feed,
            "subscriber joined"
        );

        Ok(json!({
            "videoroom": "attached",
            "room": room_id,
            "id": user_id,
            "display": room.publishers.get(&feed).map(|p| p.display.clone()),
        }))
    }

    /// Notify existing publishers that a new publisher has joined.
    fn notify_publishers_in_room(
        &self,
        room_id: RoomId,
        new_user_id: u64,
        display: &Option<String>,
    ) {
        if let Some(room) = self.rooms.get(room_id) {
            let event = json!({
                "videoroom": "event",
                "room": room_id,
                "publishers": [{
                    "id": new_user_id,
                    "display": display,
                    "audio_codec": "opus",
                    "video_codec": "vp8",
                    "talking": false,
                }]
            });

            for pub_entry in room.publishers.iter() {
                if *pub_entry.key() == new_user_id {
                    continue;
                }
                if let Some(pub_session) = self.publisher_handles.get(pub_entry.key()) {
                    if let Some(ref callbacks) = self.callbacks {
                        let session = pub_session.value().clone();
                        let event_clone = event.clone();
                        let cb = Arc::clone(callbacks);
                        tokio::spawn(async move {
                            let _ = cb.push_event(&session, "", event_clone, None).await;
                        });
                    }
                }
            }
        }
    }

    /// Handle "leave" request.
    fn handle_leave(&self, session: &PluginSession) -> Result<serde_json::Value, String> {
        let key = (session.session_id, session.handle_id);
        if let Some((_, state)) = self.handles.remove(&key) {
            match state {
                HandleState::Publisher {
                    room_id, user_id, ..
                } => {
                    if let Some(room) = self.rooms.get(room_id) {
                        room.publishers.remove(&user_id);
                        // Remove all subscribers watching this feed
                        room.subscribers.retain(|_, sub| sub.feed != user_id);
                    }
                    self.publisher_handles.remove(&user_id);
                    // Notify remaining publishers
                    self.notify_publisher_left(room_id, user_id);
                    Ok(json!({"videoroom": "event", "leaving": "ok"}))
                }
                HandleState::Subscriber {
                    room_id, user_id, ..
                } => {
                    if let Some(room) = self.rooms.get(room_id) {
                        room.subscribers.remove(&user_id);
                    }
                    Ok(json!({"videoroom": "event", "leaving": "ok"}))
                }
                HandleState::None => Ok(json!({"videoroom": "event", "leaving": "ok"})),
            }
        } else {
            Err("Not in a room".into())
        }
    }

    /// Notify publishers that someone left.
    fn notify_publisher_left(&self, room_id: RoomId, user_id: u64) {
        if let Some(room) = self.rooms.get(room_id) {
            let event = json!({
                "videoroom": "event",
                "room": room_id,
                "unpublished": user_id,
            });

            for pub_entry in room.publishers.iter() {
                if let Some(pub_session) = self.publisher_handles.get(pub_entry.key()) {
                    if let Some(ref callbacks) = self.callbacks {
                        let session = pub_session.value().clone();
                        let event_clone = event.clone();
                        let cb = Arc::clone(callbacks);
                        tokio::spawn(async move {
                            let _ = cb.push_event(&session, "", event_clone, None).await;
                        });
                    }
                }
            }
        }
    }

    /// Handle "listparticipants" request.
    fn handle_list_participants(&self, room_id: RoomId) -> Result<serde_json::Value, String> {
        let room = self
            .rooms
            .get(room_id)
            .ok_or_else(|| format!("No such room {room_id}"))?;

        let participants: Vec<serde_json::Value> = room
            .publishers
            .iter()
            .map(|p| {
                json!({
                    "id": p.user_id,
                    "display": p.display,
                    "publisher": true,
                    "talking": p.talking,
                })
            })
            .collect();

        Ok(json!({
            "videoroom": "participants",
            "room": room_id,
            "participants": participants,
        }))
    }
}

#[async_trait]
impl janus_plugin_api::JanusPlugin for VideoRoomPlugin {
    fn name(&self) -> &'static str {
        "Janus VideoRoom plugin"
    }

    fn package(&self) -> &'static str {
        "janus.plugin.videoroom"
    }

    fn version(&self) -> u32 {
        1
    }

    fn version_string(&self) -> &'static str {
        "0.1.0"
    }

    fn description(&self) -> &'static str {
        "SFU-based multi-party video conferencing room."
    }

    fn author(&self) -> &'static str {
        "Steve Seguin"
    }

    async fn init(
        &mut self,
        callbacks: Arc<dyn PluginCallbacks>,
        _config_path: &Path,
    ) -> janus_plugin_api::Result<()> {
        info!("VideoRoom plugin initialized");
        self.callbacks = Some(callbacks);

        // Create default room 1234 for demos
        self.rooms.create(
            Some(1234),
            RoomConfig {
                description: "Demo Room".into(),
                max_publishers: 6,
                ..Default::default()
            },
        );
        info!("created default room 1234");

        Ok(())
    }

    async fn destroy(&mut self) -> janus_plugin_api::Result<()> {
        info!("VideoRoom plugin destroyed");
        self.handles.clear();
        self.publisher_handles.clear();
        self.callbacks = None;
        Ok(())
    }

    async fn create_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let key = (session.session_id, session.handle_id);
        self.handles.insert(key, HandleState::None);
        debug!(session = %session, "videoroom session created");
        Ok(())
    }

    async fn destroy_session(&self, session: &PluginSession) -> janus_plugin_api::Result<()> {
        let _ = self.handle_leave(session);
        let key = (session.session_id, session.handle_id);
        self.handles.remove(&key);
        debug!(session = %session, "videoroom session destroyed");
        Ok(())
    }

    fn query_session(
        &self,
        session: &PluginSession,
    ) -> janus_plugin_api::Result<serde_json::Value> {
        let key = (session.session_id, session.handle_id);
        if let Some(state) = self.handles.get(&key) {
            let info = match state.value() {
                HandleState::None => json!({"state": "none"}),
                HandleState::Publisher {
                    room_id,
                    user_id,
                    display,
                    active,
                } => json!({
                    "state": "publisher",
                    "room": room_id,
                    "id": user_id,
                    "display": display,
                    "active": active,
                }),
                HandleState::Subscriber {
                    room_id,
                    user_id,
                    feed,
                    active,
                } => json!({
                    "state": "subscriber",
                    "room": room_id,
                    "id": user_id,
                    "feed": feed,
                    "active": active,
                }),
            };
            Ok(info)
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
        let msg: VideoRoomRequest = serde_json::from_value(body)
            .map_err(|e| janus_plugin_api::Error::Plugin(e.to_string()))?;

        match msg.request.as_str() {
            "create" => {
                let config = RoomConfig {
                    description: msg.description.unwrap_or_else(|| "VideoRoom".into()),
                    max_publishers: msg.max_publishers.unwrap_or(6),
                    bitrate: msg.bitrate.unwrap_or(0),
                    is_private: msg.is_private.unwrap_or(false),
                    pin: msg.pin,
                    secret: msg.secret,
                };
                let room = self.rooms.create(msg.room, config);
                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({
                        "videoroom": "created",
                        "room": room.id,
                    }),
                    jsep: None,
                }))
            }

            "destroy" => {
                let room_id = msg
                    .room
                    .ok_or_else(|| janus_plugin_api::Error::Plugin("Missing room ID".into()))?;
                self.rooms.destroy(room_id).ok_or_else(|| {
                    janus_plugin_api::Error::Plugin(format!("No such room {room_id}"))
                })?;
                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({"videoroom": "destroyed", "room": room_id}),
                    jsep: None,
                }))
            }

            "exists" => {
                let room_id = msg
                    .room
                    .ok_or_else(|| janus_plugin_api::Error::Plugin("Missing room ID".into()))?;
                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({
                        "videoroom": "success",
                        "room": room_id,
                        "exists": self.rooms.exists(room_id),
                    }),
                    jsep: None,
                }))
            }

            "list" => {
                let rooms: Vec<serde_json::Value> = self
                    .rooms
                    .list()
                    .iter()
                    .filter(|r| !r.config.is_private)
                    .map(|r| {
                        json!({
                            "room": r.id,
                            "description": r.config.description,
                            "max_publishers": r.config.max_publishers,
                            "bitrate": r.config.bitrate,
                            "num_participants": r.publishers.len(),
                        })
                    })
                    .collect();
                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({"videoroom": "success", "list": rooms}),
                    jsep: None,
                }))
            }

            "listparticipants" => {
                let room_id = msg
                    .room
                    .ok_or_else(|| janus_plugin_api::Error::Plugin("Missing room ID".into()))?;
                let result = self
                    .handle_list_participants(room_id)
                    .map_err(janus_plugin_api::Error::Plugin)?;
                Ok(PluginResult::Ok(PluginResultPayload {
                    body: result,
                    jsep: None,
                }))
            }

            "join" => {
                let room_id = msg
                    .room
                    .ok_or_else(|| janus_plugin_api::Error::Plugin("Missing room ID".into()))?;
                let ptype = msg.ptype.as_deref().unwrap_or("publisher");

                match ptype {
                    "publisher" => {
                        let result = self
                            .handle_join_publisher(session, room_id, msg.display)
                            .map_err(janus_plugin_api::Error::Plugin)?;
                        Ok(PluginResult::Ok(PluginResultPayload {
                            body: result,
                            jsep: None,
                        }))
                    }
                    "subscriber" | "listener" => {
                        let feed = msg.feed.ok_or_else(|| {
                            janus_plugin_api::Error::Plugin("Missing feed ID for subscriber".into())
                        })?;
                        let result = self
                            .handle_join_subscriber(session, room_id, feed)
                            .map_err(janus_plugin_api::Error::Plugin)?;

                        // For subscriber, if there's a JSEP offer the core should
                        // generate. For now, signal OkWait to let core handle SDP.
                        if let Some(ref _jsep) = jsep {
                            if let Some(ref callbacks) = self.callbacks {
                                let _ = callbacks
                                    .push_event(session, transaction, result, None)
                                    .await;
                            }
                            return Ok(PluginResult::OkWait {
                                hint: Some("Subscriber attached".into()),
                            });
                        }

                        Ok(PluginResult::Ok(PluginResultPayload {
                            body: result,
                            jsep: None,
                        }))
                    }
                    _ => Err(janus_plugin_api::Error::Plugin(format!(
                        "Unknown ptype: {ptype}"
                    ))),
                }
            }

            "publish" | "configure" => {
                let key = (session.session_id, session.handle_id);
                if let Some(mut state) = self.handles.get_mut(&key) {
                    match state.value_mut() {
                        HandleState::Publisher { active, .. } => {
                            *active = true;
                        }
                        _ => {
                            return Err(janus_plugin_api::Error::Plugin("Not a publisher".into()));
                        }
                    }
                }

                let result = json!({
                    "videoroom": "event",
                    "configured": "ok",
                    "audio": msg.audio,
                    "video": msg.video,
                });

                // If there's a JSEP offer, delegate to core for WebRTC setup
                if let Some(ref offer) = jsep {
                    if offer.jsep_type == JsepType::Offer {
                        if let Some(ref callbacks) = self.callbacks {
                            let answer_jsep = Jsep {
                                jsep_type: JsepType::Answer,
                                sdp: offer.sdp.clone(),
                                trickle: offer.trickle,
                            };
                            let _ = callbacks
                                .push_event(session, transaction, result.clone(), Some(answer_jsep))
                                .await;
                        }
                        return Ok(PluginResult::OkWait {
                            hint: Some("Processing publish".into()),
                        });
                    }
                }

                Ok(PluginResult::Ok(PluginResultPayload {
                    body: result,
                    jsep: None,
                }))
            }

            "start" => {
                // Subscriber starts receiving media
                let key = (session.session_id, session.handle_id);
                if let Some(mut state) = self.handles.get_mut(&key) {
                    if let HandleState::Subscriber { active, .. } = state.value_mut() {
                        *active = true;
                    }
                }

                // If there's a JSEP answer, the subscriber's PeerConnection is ready
                if let Some(ref answer) = jsep {
                    if answer.jsep_type == JsepType::Answer {
                        // Core handles the answer processing
                    }
                }

                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({"videoroom": "event", "started": "ok"}),
                    jsep: None,
                }))
            }

            "unpublish" => {
                let key = (session.session_id, session.handle_id);
                if let Some(mut state) = self.handles.get_mut(&key) {
                    if let HandleState::Publisher {
                        active,
                        room_id,
                        user_id,
                        ..
                    } = state.value_mut()
                    {
                        *active = false;
                        self.notify_publisher_left(*room_id, *user_id);
                    }
                }
                Ok(PluginResult::Ok(PluginResultPayload {
                    body: json!({"videoroom": "event", "unpublished": "ok"}),
                    jsep: None,
                }))
            }

            "leave" => {
                let result = self
                    .handle_leave(session)
                    .map_err(janus_plugin_api::Error::Plugin)?;
                Ok(PluginResult::Ok(PluginResultPayload {
                    body: result,
                    jsep: None,
                }))
            }

            _ => Err(janus_plugin_api::Error::Plugin(format!(
                "Unknown request: {}",
                msg.request
            ))),
        }
    }

    fn setup_media(&self, session: &PluginSession) {
        let key = (session.session_id, session.handle_id);
        if let Some(mut state) = self.handles.get_mut(&key) {
            match state.value_mut() {
                HandleState::Publisher { active, .. } => *active = true,
                HandleState::Subscriber { active, .. } => *active = true,
                _ => {}
            }
        }
        debug!(session = %session, "videoroom media ready");
    }

    fn hangup_media(&self, session: &PluginSession, reason: &str) {
        let key = (session.session_id, session.handle_id);
        if let Some(mut state) = self.handles.get_mut(&key) {
            match state.value_mut() {
                HandleState::Publisher { active, .. } => *active = false,
                HandleState::Subscriber { active, .. } => *active = false,
                _ => {}
            }
        }
        debug!(session = %session, reason = reason, "videoroom media hung up");
    }

    fn incoming_rtp(&self, session: &PluginSession, packet: &RtpPacket) {
        let key = (session.session_id, session.handle_id);

        // Only publishers forward RTP
        let (room_id, user_id) = {
            if let Some(state) = self.handles.get(&key) {
                match state.value() {
                    HandleState::Publisher {
                        room_id,
                        user_id,
                        active,
                        ..
                    } => {
                        if !active {
                            return;
                        }
                        (*room_id, *user_id)
                    }
                    _ => return,
                }
            } else {
                return;
            }
        };

        // Fan out to all subscribers watching this publisher
        if let Some(room) = self.rooms.get(room_id) {
            for sub in room.subscribers.iter() {
                if sub.feed != user_id || sub.paused {
                    continue;
                }
                // Find the subscriber's handle and relay RTP to it
                for handle_entry in self.handles.iter() {
                    if let HandleState::Subscriber {
                        user_id: sub_uid,
                        active,
                        ..
                    } = handle_entry.value()
                    {
                        if *sub_uid == *sub.key() && *active {
                            let sub_session =
                                PluginSession::new(handle_entry.key().0, handle_entry.key().1);
                            if let Some(ref callbacks) = self.callbacks {
                                callbacks.relay_rtp(&sub_session, packet);
                            }
                        }
                    }
                }
            }
        }
    }

    fn incoming_rtcp(&self, session: &PluginSession, packet: &RtcpPacket) {
        // Forward RTCP similarly to RTP
        let key = (session.session_id, session.handle_id);
        let (room_id, user_id) = {
            if let Some(state) = self.handles.get(&key) {
                match state.value() {
                    HandleState::Publisher {
                        room_id,
                        user_id,
                        active,
                        ..
                    } => {
                        if !active {
                            return;
                        }
                        (*room_id, *user_id)
                    }
                    _ => return,
                }
            } else {
                return;
            }
        };

        if let Some(room) = self.rooms.get(room_id) {
            for sub in room.subscribers.iter() {
                if sub.feed != user_id || sub.paused {
                    continue;
                }
                for handle_entry in self.handles.iter() {
                    if let HandleState::Subscriber {
                        user_id: sub_uid,
                        active,
                        ..
                    } = handle_entry.value()
                    {
                        if *sub_uid == *sub.key() && *active {
                            let sub_session =
                                PluginSession::new(handle_entry.key().0, handle_entry.key().1);
                            if let Some(ref callbacks) = self.callbacks {
                                callbacks.relay_rtcp(&sub_session, packet);
                            }
                        }
                    }
                }
            }
        }
    }

    fn slow_link(&self, session: &PluginSession, uplink: bool, lost: u32) {
        warn!(
            session = %session,
            uplink = uplink,
            lost_packets = lost,
            "videoroom slow link"
        );
    }
}

/// ABI version entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_videoroom_abi_version() -> u32 {
    janus_plugin_api::PLUGIN_LOADER_ABI_VERSION
}

/// FFI entry point for dynamic loading.
#[no_mangle]
pub extern "C" fn janus_plugin_videoroom_create() -> janus_plugin_api::PluginOpaqueHandle {
    janus_plugin_api::into_ffi_plugin(VideoRoomPlugin::default())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use janus_plugin_api::JanusPlugin;
    use std::sync::atomic::AtomicU64;

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

    fn test_session(sid: u64, hid: u64) -> PluginSession {
        PluginSession::new(SessionId(sid), HandleId(hid))
    }

    #[test]
    fn plugin_metadata() {
        let plugin = VideoRoomPlugin::default();
        assert_eq!(plugin.name(), "Janus VideoRoom plugin");
        assert_eq!(plugin.package(), "janus.plugin.videoroom");
        assert!(!plugin.description().is_empty());
    }

    #[tokio::test]
    async fn init_creates_default_room() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();
        assert!(plugin.rooms.exists(1234));
    }

    #[tokio::test]
    async fn create_and_destroy_room() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "create", "description": "Test Room", "room": 5678}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["videoroom"], "created");
                assert_eq!(payload.body["room"], 5678);
            }
            _ => panic!("expected Ok"),
        }

        assert!(plugin.rooms.exists(5678));

        // Destroy the room
        let result = plugin
            .handle_message(
                &session,
                "t2",
                json!({"request": "destroy", "room": 5678}),
                None,
            )
            .await
            .unwrap();
        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["videoroom"], "destroyed");
            }
            _ => panic!("expected Ok"),
        }
        assert!(!plugin.rooms.exists(5678));
    }

    #[tokio::test]
    async fn list_rooms() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(&session, "t1", json!({"request": "list"}), None)
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["videoroom"], "success");
                let list = payload.body["list"].as_array().unwrap();
                assert!(!list.is_empty()); // at least default room 1234
            }
            _ => panic!("expected Ok"),
        }
    }

    #[tokio::test]
    async fn check_room_exists() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "exists", "room": 1234}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["exists"], true);
            }
            _ => panic!("expected Ok"),
        }

        let result = plugin
            .handle_message(
                &session,
                "t2",
                json!({"request": "exists", "room": 9999}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["exists"], false);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[tokio::test]
    async fn join_as_publisher() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t1",
                json!({
                    "request": "join",
                    "room": 1234,
                    "ptype": "publisher",
                    "display": "Alice"
                }),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["videoroom"], "joined");
                assert_eq!(payload.body["room"], 1234);
                assert!(payload.body["id"].as_u64().is_some());
            }
            _ => panic!("expected Ok"),
        }

        // Should have one publisher in room 1234
        let room = plugin.rooms.get(1234).unwrap();
        assert_eq!(room.publishers.len(), 1);
    }

    #[tokio::test]
    async fn join_as_subscriber() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        // First, join as publisher
        let pub_session = test_session(1, 1);
        plugin.create_session(&pub_session).await.unwrap();
        let pub_result = plugin
            .handle_message(
                &pub_session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher"}),
                None,
            )
            .await
            .unwrap();
        let pub_user_id = match pub_result {
            PluginResult::Ok(p) => p.body["id"].as_u64().unwrap(),
            _ => panic!("expected Ok"),
        };

        // Now join as subscriber
        let sub_session = test_session(2, 2);
        plugin.create_session(&sub_session).await.unwrap();
        let result = plugin
            .handle_message(
                &sub_session,
                "t2",
                json!({
                    "request": "join",
                    "room": 1234,
                    "ptype": "subscriber",
                    "feed": pub_user_id
                }),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["videoroom"], "attached");
            }
            _ => panic!("expected Ok"),
        }

        let room = plugin.rooms.get(1234).unwrap();
        assert_eq!(room.subscribers.len(), 1);
    }

    #[tokio::test]
    async fn publisher_leave() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();
        plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher"}),
                None,
            )
            .await
            .unwrap();

        let room = plugin.rooms.get(1234).unwrap();
        assert_eq!(room.publishers.len(), 1);

        // Leave
        let result = plugin
            .handle_message(&session, "t2", json!({"request": "leave"}), None)
            .await
            .unwrap();
        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["videoroom"], "event");
                assert_eq!(payload.body["leaving"], "ok");
            }
            _ => panic!("expected Ok"),
        }

        assert_eq!(room.publishers.len(), 0);
    }

    #[tokio::test]
    async fn configure_publisher() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();
        plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher"}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t2",
                json!({"request": "configure", "audio": true, "video": true}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["configured"], "ok");
            }
            _ => panic!("expected Ok"),
        }
    }

    #[tokio::test]
    async fn list_participants() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();
        plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher", "display": "Alice"}),
                None,
            )
            .await
            .unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t2",
                json!({"request": "listparticipants", "room": 1234}),
                None,
            )
            .await
            .unwrap();

        match result {
            PluginResult::Ok(payload) => {
                assert_eq!(payload.body["videoroom"], "participants");
                let participants = payload.body["participants"].as_array().unwrap();
                assert_eq!(participants.len(), 1);
                assert_eq!(participants[0]["display"], "Alice");
            }
            _ => panic!("expected Ok"),
        }
    }

    #[tokio::test]
    async fn rtp_fanout_to_subscriber() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        // Publisher joins
        let pub_session = test_session(1, 1);
        plugin.create_session(&pub_session).await.unwrap();
        let pub_result = plugin
            .handle_message(
                &pub_session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher"}),
                None,
            )
            .await
            .unwrap();
        let pub_user_id = match pub_result {
            PluginResult::Ok(p) => p.body["id"].as_u64().unwrap(),
            _ => panic!("expected Ok"),
        };

        // Configure publisher (make active)
        plugin
            .handle_message(&pub_session, "t2", json!({"request": "configure"}), None)
            .await
            .unwrap();
        plugin.setup_media(&pub_session);

        // Subscriber joins
        let sub_session = test_session(2, 2);
        plugin.create_session(&sub_session).await.unwrap();
        plugin
            .handle_message(
                &sub_session,
                "t3",
                json!({
                    "request": "join",
                    "room": 1234,
                    "ptype": "subscriber",
                    "feed": pub_user_id
                }),
                None,
            )
            .await
            .unwrap();

        // Start subscriber
        plugin
            .handle_message(&sub_session, "t4", json!({"request": "start"}), None)
            .await
            .unwrap();
        plugin.setup_media(&sub_session);

        // Send RTP from publisher
        let packet = RtpPacket::new(true, vec![0x80, 111, 0, 1, 0, 0, 0, 160, 0, 0, 3, 232]);
        plugin.incoming_rtp(&pub_session, &packet);

        // Should have been relayed to subscriber
        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn rtp_not_relayed_when_no_subscribers() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        let cb_clone = Arc::clone(&cb);
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let pub_session = test_session(1, 1);
        plugin.create_session(&pub_session).await.unwrap();
        plugin
            .handle_message(
                &pub_session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher"}),
                None,
            )
            .await
            .unwrap();
        plugin
            .handle_message(&pub_session, "t2", json!({"request": "configure"}), None)
            .await
            .unwrap();
        plugin.setup_media(&pub_session);

        let packet = RtpPacket::new(false, vec![0x80, 111, 0, 1]);
        plugin.incoming_rtp(&pub_session, &packet);

        assert_eq!(cb_clone.rtp_relayed.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn destroy_session_cleans_up_publisher() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();
        plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher"}),
                None,
            )
            .await
            .unwrap();

        let room = plugin.rooms.get(1234).unwrap();
        assert_eq!(room.publishers.len(), 1);

        plugin.destroy_session(&session).await.unwrap();
        assert_eq!(room.publishers.len(), 0);
    }

    #[tokio::test]
    async fn unknown_request_errors() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(&session, "t1", json!({"request": "bogus"}), None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn subscriber_to_nonexistent_feed_errors() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t1",
                json!({
                    "request": "join",
                    "room": 1234,
                    "ptype": "subscriber",
                    "feed": 999999
                }),
                None,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn join_nonexistent_room_errors() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        let result = plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "join", "room": 9999, "ptype": "publisher"}),
                None,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn query_session_shows_state() {
        let mut plugin = VideoRoomPlugin::default();
        let cb = Arc::new(MockCallbacks::new());
        plugin.init(cb, Path::new("/tmp")).await.unwrap();

        let session = test_session(1, 1);
        plugin.create_session(&session).await.unwrap();

        // Before joining
        let info = plugin.query_session(&session).unwrap();
        assert_eq!(info["state"], "none");

        // After joining
        plugin
            .handle_message(
                &session,
                "t1",
                json!({"request": "join", "room": 1234, "ptype": "publisher"}),
                None,
            )
            .await
            .unwrap();

        let info = plugin.query_session(&session).unwrap();
        assert_eq!(info["state"], "publisher");
        assert_eq!(info["room"], 1234);
    }

    #[test]
    fn ffi_create_returns_valid_plugin() {
        let raw = janus_plugin_videoroom_create();
        assert!(!raw.is_null());
        unsafe {
            let _ = janus_plugin_api::from_ffi_plugin(raw);
        }
    }
}
