//! VideoRoom room management.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Unique room identifier.
pub type RoomId = u64;

/// Global counter for auto-generating room IDs.
static NEXT_ROOM_ID: AtomicU64 = AtomicU64::new(1000);

/// Configuration for a video room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomConfig {
    pub description: String,
    pub max_publishers: u32,
    pub bitrate: u64,
    pub is_private: bool,
    pub pin: Option<String>,
    pub secret: Option<String>,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            description: "VideoRoom".into(),
            max_publishers: 6,
            bitrate: 0,
            is_private: false,
            pin: None,
            secret: None,
        }
    }
}

/// A video room instance.
#[derive(Debug)]
pub struct Room {
    pub id: RoomId,
    pub config: RoomConfig,
    /// Publisher user IDs currently in the room.
    pub publishers: DashMap<u64, PublisherInfo>,
    /// Subscriber feed IDs currently in the room.
    pub subscribers: DashMap<u64, SubscriberInfo>,
}

/// Information about a publisher in a room.
#[derive(Debug, Clone, Serialize)]
pub struct PublisherInfo {
    pub user_id: u64,
    pub display: Option<String>,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub talking: bool,
}

/// Information about a subscriber in a room.
#[derive(Debug, Clone)]
pub struct SubscriberInfo {
    pub user_id: u64,
    /// The publisher feed this subscriber is watching.
    pub feed: u64,
    pub paused: bool,
}

/// Registry of all rooms.
pub struct RoomRegistry {
    rooms: DashMap<RoomId, Arc<Room>>,
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self {
            rooms: DashMap::new(),
        }
    }

    /// Create a new room with an explicit ID or auto-generated.
    pub fn create(&self, id: Option<RoomId>, config: RoomConfig) -> Arc<Room> {
        let room_id = id.unwrap_or_else(|| NEXT_ROOM_ID.fetch_add(1, Ordering::Relaxed));
        let room = Arc::new(Room {
            id: room_id,
            config,
            publishers: DashMap::new(),
            subscribers: DashMap::new(),
        });
        self.rooms.insert(room_id, Arc::clone(&room));
        room
    }

    /// Get a room by ID.
    pub fn get(&self, id: RoomId) -> Option<Arc<Room>> {
        self.rooms.get(&id).map(|r| Arc::clone(r.value()))
    }

    /// Check if a room exists.
    pub fn exists(&self, id: RoomId) -> bool {
        self.rooms.contains_key(&id)
    }

    /// Destroy a room by ID.
    pub fn destroy(&self, id: RoomId) -> Option<Arc<Room>> {
        self.rooms.remove(&id).map(|(_, r)| r)
    }

    /// List all rooms.
    pub fn list(&self) -> Vec<Arc<Room>> {
        self.rooms.iter().map(|r| Arc::clone(r.value())).collect()
    }

    /// Number of rooms.
    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}

impl Default for RoomRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_room_with_defaults() {
        let registry = RoomRegistry::new();
        let room = registry.create(Some(1234), RoomConfig::default());
        assert_eq!(room.id, 1234);
        assert_eq!(room.config.max_publishers, 6);
        assert!(room.publishers.is_empty());
    }

    #[test]
    fn create_room_auto_id() {
        let registry = RoomRegistry::new();
        let room = registry.create(None, RoomConfig::default());
        assert!(room.id >= 1000);
    }

    #[test]
    fn get_room() {
        let registry = RoomRegistry::new();
        registry.create(Some(42), RoomConfig::default());
        assert!(registry.get(42).is_some());
        assert!(registry.get(99).is_none());
    }

    #[test]
    fn destroy_room() {
        let registry = RoomRegistry::new();
        registry.create(Some(42), RoomConfig::default());
        assert!(registry.exists(42));
        registry.destroy(42);
        assert!(!registry.exists(42));
    }

    #[test]
    fn list_rooms() {
        let registry = RoomRegistry::new();
        registry.create(Some(1), RoomConfig::default());
        registry.create(Some(2), RoomConfig::default());
        registry.create(Some(3), RoomConfig::default());
        assert_eq!(registry.list().len(), 3);
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn room_publishers_and_subscribers() {
        let registry = RoomRegistry::new();
        let room = registry.create(Some(1), RoomConfig::default());
        room.publishers.insert(
            100,
            PublisherInfo {
                user_id: 100,
                display: Some("Alice".into()),
                audio_codec: Some("opus".into()),
                video_codec: Some("vp8".into()),
                talking: false,
            },
        );
        room.subscribers.insert(
            200,
            SubscriberInfo {
                user_id: 200,
                feed: 100,
                paused: false,
            },
        );
        assert_eq!(room.publishers.len(), 1);
        assert_eq!(room.subscribers.len(), 1);
    }

    #[test]
    fn room_config_serialization() {
        let config = RoomConfig {
            description: "Test Room".into(),
            max_publishers: 10,
            bitrate: 256000,
            is_private: true,
            pin: Some("1234".into()),
            secret: Some("admin".into()),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["max_publishers"], 10);
        assert_eq!(json["bitrate"], 256000);
    }
}
