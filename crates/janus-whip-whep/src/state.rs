//! Shared state for WHIP/WHEP resources.

use crate::fanout::FanOut;
use crate::link_headers::IceServer;
use dashmap::DashMap;
use janus_core::config::NatConfig;
use janus_core::webrtc::PeerConnectionHandle;
use janus_plugin_api::uuid_v4_simple;
use std::sync::Arc;
use std::time::Instant;

/// Unique identifier for a WHIP/WHEP resource.
/// Stored as 16 raw bytes (UUID v4), displayed as hyphenated hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId([u8; 16]);

impl Default for ResourceId {
    fn default() -> Self {
        let mut b = [0u8; 16];
        getrandom::getrandom(&mut b).expect("getrandom failed");
        // Set version (4) and variant (RFC 4122)
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        Self(b)
    }
}

impl ResourceId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }
}

impl std::str::FromStr for ResourceId {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex: String = s.chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid resource ID format");
        }
        let mut b = [0u8; 16];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            b[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16)
                .map_err(|_| "invalid hex")?;
        }
        Ok(Self(b))
    }
}

/// Whether a resource is a publisher (WHIP), subscriber (WHEP), or outgoing relay (WhipOut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Whip,
    Whep,
    /// Outgoing WHIP relay — our server pushes media to a remote WHIP endpoint.
    WhipOut,
}

/// A WHIP, WHEP, or WHIP-out resource representing a live PeerConnection.
pub struct Resource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    /// ETag for ICE session identity (used for trickle / ICE restart).
    pub etag: String,
    pub pc_handle: PeerConnectionHandle,
    pub created_at: Instant,
    /// For WHEP/WhipOut resources: which WHIP publisher this subscribes to.
    pub publisher_id: Option<ResourceId>,
    /// For WhipOut resources: the remote resource URL (from Location header).
    pub remote_resource_url: Option<String>,
    /// For WhipOut resources: the bearer token for the remote endpoint.
    pub remote_bearer_token: Option<String>,
    /// For WhipOut resources: the original endpoint URL.
    pub remote_endpoint: Option<String>,
}

/// Shared state for WHIP/WHEP handlers.
#[derive(Clone)]
pub struct WhipWhepState {
    /// All active resources indexed by ResourceId.
    pub resources: Arc<DashMap<ResourceId, Resource>>,
    /// Fan-out manager for relaying media from publishers to subscribers.
    pub fanout: Arc<FanOut>,
    /// ICE servers derived from NatConfig (for Link headers).
    pub ice_servers: Arc<Vec<IceServer>>,
    /// Whether to use ICE-lite for new PeerConnections.
    pub ice_lite: bool,
}

impl WhipWhepState {
    /// Create a new state from the server's NAT configuration.
    pub fn new(nat_config: &NatConfig) -> Self {
        let ice_servers = crate::link_headers::ice_servers_from_nat(nat_config);
        Self {
            resources: Arc::new(DashMap::new()),
            fanout: Arc::new(FanOut::new()),
            ice_servers: Arc::new(ice_servers),
            ice_lite: nat_config.ice_lite,
        }
    }
}

/// Generate a random ETag string for ICE session identity.
pub fn generate_etag() -> String {
    uuid_v4_simple()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_id_roundtrip() {
        let id = ResourceId::new();
        let s = id.to_string();
        let parsed: ResourceId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn resource_id_display() {
        let id = ResourceId::new();
        let s = id.to_string();
        assert_eq!(s.len(), 36); // UUID with hyphens
    }

    #[test]
    fn resource_id_parse_invalid() {
        let result: Result<ResourceId, _> = "not-a-uuid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn generate_etag_is_unique() {
        let a = generate_etag();
        let b = generate_etag();
        assert_ne!(a, b);
    }

    #[test]
    fn generate_etag_is_hex() {
        let etag = generate_etag();
        assert_eq!(etag.len(), 32); // UUID simple format = 32 hex chars
        assert!(etag.chars().all(|c| c.is_ascii_hexdigit()));
    }

    fn empty_nat() -> NatConfig {
        NatConfig {
            stun_server: None,
            stun_port: 3478,
            ..NatConfig::default()
        }
    }

    #[test]
    fn whip_whep_state_default_nat() {
        let nat = NatConfig::default();
        let state = WhipWhepState::new(&nat);
        assert_eq!(state.resources.len(), 0);
        // Default NatConfig includes Google STUN
        assert_eq!(state.ice_servers.len(), 1);
        assert!(!state.ice_lite);
    }

    #[test]
    fn whip_whep_state_empty_nat() {
        let nat = empty_nat();
        let state = WhipWhepState::new(&nat);
        assert!(state.ice_servers.is_empty());
    }

    #[test]
    fn whip_whep_state_with_stun() {
        let mut nat = empty_nat();
        nat.stun_server = Some("stun.example.com".into());
        nat.stun_port = 3478;
        let state = WhipWhepState::new(&nat);
        assert_eq!(state.ice_servers.len(), 1);
    }

    #[test]
    fn whip_whep_state_with_turn() {
        let mut nat = empty_nat();
        nat.turn_server = Some("turn.example.com".into());
        nat.turn_port = 3478;
        nat.turn_user = Some("user".into());
        nat.turn_pwd = Some("pass".into());
        let state = WhipWhepState::new(&nat);
        assert_eq!(state.ice_servers.len(), 1);
    }

    #[test]
    fn whip_whep_state_with_stun_and_turn() {
        let mut nat = empty_nat();
        nat.stun_server = Some("stun.example.com".into());
        nat.turn_server = Some("turn.example.com".into());
        nat.turn_user = Some("u".into());
        nat.turn_pwd = Some("p".into());
        let state = WhipWhepState::new(&nat);
        assert_eq!(state.ice_servers.len(), 2);
    }

    #[test]
    fn whip_whep_state_ice_lite() {
        let mut nat = NatConfig::default();
        nat.ice_lite = true;
        let state = WhipWhepState::new(&nat);
        assert!(state.ice_lite);
    }

    #[test]
    fn resource_kind_equality() {
        assert_eq!(ResourceKind::Whip, ResourceKind::Whip);
        assert_eq!(ResourceKind::Whep, ResourceKind::Whep);
        assert_ne!(ResourceKind::Whip, ResourceKind::Whep);
    }
}
