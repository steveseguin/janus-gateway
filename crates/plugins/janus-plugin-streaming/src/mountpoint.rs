//! Streaming mountpoint management.

use dashmap::DashMap;
use janus_plugin_api::HandleId;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Unique mountpoint identifier.
pub type MountpointId = u64;

/// Global counter for auto-generating mountpoint IDs.
static NEXT_MOUNTPOINT_ID: AtomicU64 = AtomicU64::new(100);

/// Configuration for a streaming mountpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountpointConfig {
    pub name: String,
    pub description: String,
    pub is_private: bool,
    pub pin: Option<String>,
    pub secret: Option<String>,
    /// UDP port to receive audio RTP on (0 = disabled).
    pub audio_port: u16,
    /// UDP port to receive video RTP on (0 = disabled).
    pub video_port: u16,
    /// Audio codec (e.g., "opus").
    pub audio_codec: Option<String>,
    /// Video codec (e.g., "vp8").
    pub video_codec: Option<String>,
    /// Audio RTP payload type.
    pub audio_pt: Option<u8>,
    /// Video RTP payload type.
    pub video_pt: Option<u8>,
}

impl Default for MountpointConfig {
    fn default() -> Self {
        Self {
            name: "Stream".into(),
            description: "Streaming mountpoint".into(),
            is_private: false,
            pin: None,
            secret: None,
            audio_port: 0,
            video_port: 0,
            audio_codec: Some("opus".into()),
            video_codec: Some("vp8".into()),
            audio_pt: Some(111),
            video_pt: Some(96),
        }
    }
}

/// A streaming mountpoint instance.
#[derive(Debug)]
pub struct Mountpoint {
    pub id: MountpointId,
    pub config: MountpointConfig,
    /// Whether this mountpoint is actively receiving RTP.
    pub active: AtomicBool,
    /// Viewer handles currently watching this mountpoint.
    pub viewers: DashMap<HandleId, ViewerInfo>,
}

/// Information about a viewer watching a mountpoint.
#[derive(Debug, Clone)]
pub struct ViewerInfo {
    pub handle_id: HandleId,
    pub paused: bool,
}

/// Registry of all mountpoints.
pub struct MountpointRegistry {
    mountpoints: DashMap<MountpointId, Arc<Mountpoint>>,
}

impl MountpointRegistry {
    pub fn new() -> Self {
        Self {
            mountpoints: DashMap::new(),
        }
    }

    /// Create a new mountpoint with an explicit ID or auto-generated.
    pub fn create(&self, id: Option<MountpointId>, config: MountpointConfig) -> Arc<Mountpoint> {
        let mp_id = id.unwrap_or_else(|| NEXT_MOUNTPOINT_ID.fetch_add(1, Ordering::Relaxed));
        let mp = Arc::new(Mountpoint {
            id: mp_id,
            config,
            active: AtomicBool::new(false),
            viewers: DashMap::new(),
        });
        self.mountpoints.insert(mp_id, Arc::clone(&mp));
        mp
    }

    /// Get a mountpoint by ID.
    pub fn get(&self, id: MountpointId) -> Option<Arc<Mountpoint>> {
        self.mountpoints.get(&id).map(|r| Arc::clone(r.value()))
    }

    /// Check if a mountpoint exists.
    pub fn exists(&self, id: MountpointId) -> bool {
        self.mountpoints.contains_key(&id)
    }

    /// Destroy a mountpoint by ID.
    pub fn destroy(&self, id: MountpointId) -> Option<Arc<Mountpoint>> {
        self.mountpoints.remove(&id).map(|(_, r)| r)
    }

    /// List all mountpoints.
    pub fn list(&self) -> Vec<Arc<Mountpoint>> {
        self.mountpoints
            .iter()
            .map(|r| Arc::clone(r.value()))
            .collect()
    }

    /// Number of mountpoints.
    pub fn len(&self) -> usize {
        self.mountpoints.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.mountpoints.is_empty()
    }
}

impl Default for MountpointRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_mountpoint_with_defaults() {
        let registry = MountpointRegistry::new();
        let mp = registry.create(Some(1), MountpointConfig::default());
        assert_eq!(mp.id, 1);
        assert_eq!(mp.config.name, "Stream");
        assert!(mp.viewers.is_empty());
    }

    #[test]
    fn create_mountpoint_auto_id() {
        let registry = MountpointRegistry::new();
        let mp = registry.create(None, MountpointConfig::default());
        assert!(mp.id >= 100);
    }

    #[test]
    fn get_mountpoint() {
        let registry = MountpointRegistry::new();
        registry.create(Some(42), MountpointConfig::default());
        assert!(registry.get(42).is_some());
        assert!(registry.get(99).is_none());
    }

    #[test]
    fn destroy_mountpoint() {
        let registry = MountpointRegistry::new();
        registry.create(Some(42), MountpointConfig::default());
        assert!(registry.exists(42));
        registry.destroy(42);
        assert!(!registry.exists(42));
    }

    #[test]
    fn list_mountpoints() {
        let registry = MountpointRegistry::new();
        registry.create(Some(1), MountpointConfig::default());
        registry.create(Some(2), MountpointConfig::default());
        registry.create(Some(3), MountpointConfig::default());
        assert_eq!(registry.list().len(), 3);
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn mountpoint_viewers() {
        let registry = MountpointRegistry::new();
        let mp = registry.create(Some(1), MountpointConfig::default());
        mp.viewers.insert(
            HandleId(100),
            ViewerInfo {
                handle_id: HandleId(100),
                paused: false,
            },
        );
        assert_eq!(mp.viewers.len(), 1);
    }

    #[test]
    fn mountpoint_config_serialization() {
        let config = MountpointConfig {
            name: "Test Stream".into(),
            description: "Test".into(),
            is_private: true,
            pin: Some("1234".into()),
            secret: Some("admin".into()),
            audio_port: 5004,
            video_port: 5006,
            audio_codec: Some("opus".into()),
            video_codec: Some("vp8".into()),
            audio_pt: Some(111),
            video_pt: Some(96),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["audio_port"], 5004);
        assert_eq!(json["video_port"], 5006);
    }
}
