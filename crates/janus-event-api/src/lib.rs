//! Event handler API for Janus Gateway.
//!
//! Event handlers subscribe to internal Janus events and forward them to
//! external systems (MQTT, RabbitMQ, Graylog, etc.).

use async_trait::async_trait;
use janus_plugin_api::SessionId;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// API version that event handlers must be compatible with.
pub const EVENT_HANDLER_API_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Event handler trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait JanusEventHandler: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn package(&self) -> &'static str;
    fn version(&self) -> u32;
    fn version_string(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn author(&self) -> &'static str;

    /// Which event types this handler subscribes to.
    fn events_mask(&self) -> EventMask;

    async fn init(&mut self, config_path: &Path) -> Result<()>;
    async fn destroy(&mut self) -> Result<()>;

    /// Handle an incoming event. Called on the event dispatch thread.
    async fn handle_event(&self, event: &JanusEvent) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// Bitmask of event types a handler subscribes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventMask(pub u32);

impl EventMask {
    pub const NONE: Self = Self(0);
    pub const SESSION: Self = Self(1 << 0);
    pub const HANDLE: Self = Self(1 << 1);
    pub const JSEP: Self = Self(1 << 2);
    pub const WEBRTC: Self = Self(1 << 3);
    pub const MEDIA: Self = Self(1 << 4);
    pub const PLUGIN: Self = Self(1 << 5);
    pub const TRANSPORT: Self = Self(1 << 6);
    pub const CORE: Self = Self(1 << 7);
    pub const ALL: Self = Self(0xFF);

    /// Check whether a specific event type is included in this mask.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Combine two masks.
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// A Janus event that event handlers receive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JanusEvent {
    /// The event type.
    #[serde(rename = "type")]
    pub event_type: EventType,
    /// Timestamp (microseconds since epoch).
    pub timestamp: u64,
    /// Session ID (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Event-specific payload.
    pub event: serde_json::Value,
}

/// Enumeration of event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Session,
    Handle,
    Jsep,
    WebRTC,
    Media,
    Plugin,
    Transport,
    Core,
}

impl EventType {
    /// Convert to the corresponding [`EventMask`] bit.
    pub fn to_mask(self) -> EventMask {
        match self {
            EventType::Session => EventMask::SESSION,
            EventType::Handle => EventMask::HANDLE,
            EventType::Jsep => EventMask::JSEP,
            EventType::WebRTC => EventMask::WEBRTC,
            EventType::Media => EventMask::MEDIA,
            EventType::Plugin => EventMask::PLUGIN,
            EventType::Transport => EventMask::TRANSPORT,
            EventType::Core => EventMask::CORE,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum Error {
    #[error("event handler error: {0}")]
    Handler(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_mask_contains() {
        let mask = EventMask::SESSION.union(EventMask::MEDIA);
        assert!(mask.contains(EventMask::SESSION));
        assert!(mask.contains(EventMask::MEDIA));
        assert!(!mask.contains(EventMask::PLUGIN));
    }

    #[test]
    fn event_mask_all_contains_everything() {
        assert!(EventMask::ALL.contains(EventMask::SESSION));
        assert!(EventMask::ALL.contains(EventMask::HANDLE));
        assert!(EventMask::ALL.contains(EventMask::JSEP));
        assert!(EventMask::ALL.contains(EventMask::WEBRTC));
        assert!(EventMask::ALL.contains(EventMask::MEDIA));
        assert!(EventMask::ALL.contains(EventMask::PLUGIN));
        assert!(EventMask::ALL.contains(EventMask::TRANSPORT));
        assert!(EventMask::ALL.contains(EventMask::CORE));
    }

    #[test]
    fn event_mask_none_contains_nothing() {
        assert!(!EventMask::NONE.contains(EventMask::SESSION));
    }

    #[test]
    fn event_type_to_mask_roundtrip() {
        let types = [
            EventType::Session,
            EventType::Handle,
            EventType::Jsep,
            EventType::WebRTC,
            EventType::Media,
            EventType::Plugin,
            EventType::Transport,
            EventType::Core,
        ];
        for t in types {
            let mask = t.to_mask();
            assert!(mask.contains(t.to_mask()));
        }
    }

    #[test]
    fn janus_event_serialize() {
        let event = JanusEvent {
            event_type: EventType::Session,
            timestamp: 1000000,
            session_id: Some(SessionId(42)),
            event: serde_json::json!({"name": "created"}),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "session");
        assert_eq!(json["timestamp"], 1000000);
        assert_eq!(json["session_id"], 42);
    }

    #[test]
    fn janus_event_deserialize() {
        let json = r#"{
            "type": "media",
            "timestamp": 999,
            "event": {"receiving": true}
        }"#;
        let event: JanusEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event_type, EventType::Media);
        assert_eq!(event.timestamp, 999);
        assert!(event.session_id.is_none());
    }

    #[test]
    fn event_type_serde_roundtrip() {
        let types = [
            EventType::Session,
            EventType::Handle,
            EventType::Plugin,
            EventType::Core,
        ];
        for t in types {
            let json = serde_json::to_string(&t).unwrap();
            let parsed: EventType = serde_json::from_str(&json).unwrap();
            assert_eq!(t, parsed);
        }
    }
}
