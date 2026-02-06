//! Core types shared between plugins and the Janus core.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Unique identifier for a Janus session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u64);

impl SessionId {
    /// Generate a new random session ID.
    pub fn random() -> Self {
        // Use the lower 64 bits of a UUID v4 to get a random u64.
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let val = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        Self(val)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a plugin handle within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HandleId(pub u64);

impl HandleId {
    /// Generate a new random handle ID.
    pub fn random() -> Self {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let val = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        Self(val)
    }
}

impl fmt::Display for HandleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Plugin session
// ---------------------------------------------------------------------------

/// Opaque session handle passed between the core and a plugin.
///
/// Contains identifiers the plugin needs to address callbacks back to the core.
#[derive(Debug, Clone)]
pub struct PluginSession {
    /// The Janus session this handle belongs to.
    pub session_id: SessionId,
    /// The handle ID within the session.
    pub handle_id: HandleId,
}

impl PluginSession {
    pub fn new(session_id: SessionId, handle_id: HandleId) -> Self {
        Self {
            session_id,
            handle_id,
        }
    }
}

impl fmt::Display for PluginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.session_id, self.handle_id)
    }
}

// ---------------------------------------------------------------------------
// JSEP (JavaScript Session Establishment Protocol)
// ---------------------------------------------------------------------------

/// SDP offer or answer exchanged during WebRTC negotiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jsep {
    /// "offer" or "answer"
    #[serde(rename = "type")]
    pub jsep_type: JsepType,
    /// The SDP string.
    pub sdp: String,
    /// Whether to trickle ICE candidates (default: true).
    #[serde(default = "default_trickle")]
    pub trickle: bool,
}

fn default_trickle() -> bool {
    true
}

/// JSEP type: offer or answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsepType {
    Offer,
    Answer,
}

impl fmt::Display for JsepType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsepType::Offer => write!(f, "offer"),
            JsepType::Answer => write!(f, "answer"),
        }
    }
}

// ---------------------------------------------------------------------------
// RTP / RTCP packets
// ---------------------------------------------------------------------------

/// An RTP packet with metadata.
#[derive(Debug, Clone)]
pub struct RtpPacket {
    /// Whether this is a video packet (false = audio).
    pub video: bool,
    /// Raw RTP bytes (header + payload).
    pub buffer: Vec<u8>,
}

impl RtpPacket {
    pub fn new(video: bool, buffer: Vec<u8>) -> Self {
        Self { video, buffer }
    }

    /// Read the payload type from the RTP header (bits 1-7 of byte 1).
    pub fn payload_type(&self) -> Option<u8> {
        self.buffer.get(1).map(|b| b & 0x7F)
    }

    /// Read the sequence number from the RTP header (bytes 2-3).
    pub fn sequence_number(&self) -> Option<u16> {
        if self.buffer.len() >= 4 {
            Some(u16::from_be_bytes([self.buffer[2], self.buffer[3]]))
        } else {
            None
        }
    }

    /// Read the SSRC from the RTP header (bytes 8-11).
    pub fn ssrc(&self) -> Option<u32> {
        if self.buffer.len() >= 12 {
            Some(u32::from_be_bytes([
                self.buffer[8],
                self.buffer[9],
                self.buffer[10],
                self.buffer[11],
            ]))
        } else {
            None
        }
    }

    /// Read the timestamp from the RTP header (bytes 4-7).
    pub fn timestamp(&self) -> Option<u32> {
        if self.buffer.len() >= 8 {
            Some(u32::from_be_bytes([
                self.buffer[4],
                self.buffer[5],
                self.buffer[6],
                self.buffer[7],
            ]))
        } else {
            None
        }
    }
}

/// An RTCP packet.
#[derive(Debug, Clone)]
pub struct RtcpPacket {
    /// Whether this relates to video (false = audio).
    pub video: bool,
    /// Raw RTCP bytes.
    pub buffer: Vec<u8>,
}

impl RtcpPacket {
    pub fn new(video: bool, buffer: Vec<u8>) -> Self {
        Self { video, buffer }
    }
}

// ---------------------------------------------------------------------------
// Plugin results
// ---------------------------------------------------------------------------

/// Result type returned by `handle_message`.
#[derive(Debug)]
pub enum PluginResult {
    /// Reply immediately with this JSON body (+ optional JSEP).
    Ok(PluginResultPayload),
    /// Processing will happen asynchronously — the plugin will call
    /// `push_event` later.
    OkWait {
        hint: Option<String>,
    },
}

/// Synchronous plugin response.
#[derive(Debug, Serialize)]
pub struct PluginResultPayload {
    pub body: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsep: Option<Jsep>,
}

// ---------------------------------------------------------------------------
// Media direction
// ---------------------------------------------------------------------------

/// Media direction for SDP negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaDirection {
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
}

impl fmt::Display for MediaDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaDirection::SendRecv => write!(f, "sendrecv"),
            MediaDirection::SendOnly => write!(f, "sendonly"),
            MediaDirection::RecvOnly => write!(f, "recvonly"),
            MediaDirection::Inactive => write!(f, "inactive"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_display() {
        let id = SessionId(12345);
        assert_eq!(id.to_string(), "12345");
    }

    #[test]
    fn session_id_random_is_unique() {
        let a = SessionId::random();
        let b = SessionId::random();
        assert_ne!(a, b);
    }

    #[test]
    fn handle_id_display() {
        let id = HandleId(67890);
        assert_eq!(id.to_string(), "67890");
    }

    #[test]
    fn plugin_session_display() {
        let ps = PluginSession::new(SessionId(1), HandleId(2));
        assert_eq!(ps.to_string(), "1:2");
    }

    #[test]
    fn jsep_serialize_roundtrip() {
        let jsep = Jsep {
            jsep_type: JsepType::Offer,
            sdp: "v=0\r\n".into(),
            trickle: true,
        };
        let json = serde_json::to_string(&jsep).unwrap();
        let parsed: Jsep = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.jsep_type, JsepType::Offer);
        assert_eq!(parsed.sdp, "v=0\r\n");
        assert!(parsed.trickle);
    }

    #[test]
    fn jsep_deserialize_defaults_trickle_true() {
        let json = r#"{"type": "answer", "sdp": "v=0\r\n"}"#;
        let jsep: Jsep = serde_json::from_str(json).unwrap();
        assert_eq!(jsep.jsep_type, JsepType::Answer);
        assert!(jsep.trickle);
    }

    #[test]
    fn jsep_type_display() {
        assert_eq!(JsepType::Offer.to_string(), "offer");
        assert_eq!(JsepType::Answer.to_string(), "answer");
    }

    #[test]
    fn rtp_packet_parse_header() {
        // Minimal valid RTP header: V=2, PT=111, seq=1, ts=160, ssrc=1000
        let mut buf = vec![0u8; 12];
        buf[0] = 0x80; // V=2
        buf[1] = 111; // PT=111 (Opus)
        buf[2..4].copy_from_slice(&1u16.to_be_bytes()); // seq=1
        buf[4..8].copy_from_slice(&160u32.to_be_bytes()); // ts=160
        buf[8..12].copy_from_slice(&1000u32.to_be_bytes()); // ssrc=1000

        let pkt = RtpPacket::new(false, buf);
        assert_eq!(pkt.payload_type(), Some(111));
        assert_eq!(pkt.sequence_number(), Some(1));
        assert_eq!(pkt.timestamp(), Some(160));
        assert_eq!(pkt.ssrc(), Some(1000));
        assert!(!pkt.video);
    }

    #[test]
    fn rtp_packet_short_buffer_returns_none() {
        let pkt = RtpPacket::new(true, vec![0x80]);
        assert_eq!(pkt.payload_type(), None);
        assert_eq!(pkt.sequence_number(), None);
        assert_eq!(pkt.timestamp(), None);
        assert_eq!(pkt.ssrc(), None);
    }

    #[test]
    fn media_direction_display() {
        assert_eq!(MediaDirection::SendRecv.to_string(), "sendrecv");
        assert_eq!(MediaDirection::SendOnly.to_string(), "sendonly");
        assert_eq!(MediaDirection::RecvOnly.to_string(), "recvonly");
        assert_eq!(MediaDirection::Inactive.to_string(), "inactive");
    }

    #[test]
    fn media_direction_serde_roundtrip() {
        let dir = MediaDirection::RecvOnly;
        let json = serde_json::to_string(&dir).unwrap();
        assert_eq!(json, "\"recvonly\"");
        let parsed: MediaDirection = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, dir);
    }

    #[test]
    fn session_id_zero() {
        let id = SessionId(0);
        assert_eq!(id.to_string(), "0");
        assert_eq!(id.0, 0);
    }

    #[test]
    fn session_id_max() {
        let id = SessionId(u64::MAX);
        assert_eq!(id.0, u64::MAX);
        let json = serde_json::to_string(&id).unwrap();
        let parsed: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn handle_id_random_is_unique() {
        let a = HandleId::random();
        let b = HandleId::random();
        assert_ne!(a, b);
    }

    #[test]
    fn session_id_hash_works() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(SessionId(1));
        set.insert(SessionId(2));
        set.insert(SessionId(1)); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn handle_id_hash_works() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(HandleId(10));
        set.insert(HandleId(20));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn jsep_trickle_false() {
        let json = r#"{"type": "offer", "sdp": "v=0\r\n", "trickle": false}"#;
        let jsep: Jsep = serde_json::from_str(json).unwrap();
        assert!(!jsep.trickle);
    }

    #[test]
    fn jsep_invalid_type_errors() {
        let json = r#"{"type": "invalid", "sdp": "v=0\r\n"}"#;
        let result = serde_json::from_str::<Jsep>(json);
        assert!(result.is_err());
    }

    #[test]
    fn jsep_missing_sdp_errors() {
        let json = r#"{"type": "offer"}"#;
        let result = serde_json::from_str::<Jsep>(json);
        assert!(result.is_err());
    }

    #[test]
    fn rtp_packet_empty_buffer() {
        let pkt = RtpPacket::new(false, vec![]);
        assert_eq!(pkt.payload_type(), None);
        assert_eq!(pkt.sequence_number(), None);
        assert_eq!(pkt.timestamp(), None);
        assert_eq!(pkt.ssrc(), None);
    }

    #[test]
    fn rtp_packet_marker_bit_in_payload_type() {
        // Byte 1 = 0xFF means marker=1, PT=127
        let mut buf = vec![0u8; 12];
        buf[1] = 0xFF;
        let pkt = RtpPacket::new(false, buf);
        assert_eq!(pkt.payload_type(), Some(127)); // 0x7F
    }

    #[test]
    fn rtp_packet_with_payload() {
        let mut buf = vec![0u8; 172]; // 12 header + 160 payload
        buf[0] = 0x80;
        buf[1] = 111;
        buf[2..4].copy_from_slice(&42u16.to_be_bytes());
        let pkt = RtpPacket::new(false, buf.clone());
        assert_eq!(pkt.sequence_number(), Some(42));
        assert_eq!(pkt.buffer.len(), 172);
    }

    #[test]
    fn rtcp_packet_construction() {
        let pkt = RtcpPacket::new(true, vec![1, 2, 3, 4]);
        assert!(pkt.video);
        assert_eq!(pkt.buffer, vec![1, 2, 3, 4]);
    }

    #[test]
    fn plugin_result_payload_serializes() {
        let payload = PluginResultPayload {
            body: serde_json::json!({"result": "ok"}),
            jsep: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["body"]["result"], "ok");
        assert!(json.get("jsep").is_none());
    }

    #[test]
    fn plugin_result_payload_with_jsep_serializes() {
        let payload = PluginResultPayload {
            body: serde_json::json!({"result": "ok"}),
            jsep: Some(Jsep {
                jsep_type: JsepType::Answer,
                sdp: "v=0\r\n".into(),
                trickle: true,
            }),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["jsep"]["type"], "answer");
    }

    #[test]
    fn plugin_session_clone() {
        let ps = PluginSession::new(SessionId(1), HandleId(2));
        let cloned = ps.clone();
        assert_eq!(ps.session_id, cloned.session_id);
        assert_eq!(ps.handle_id, cloned.handle_id);
    }

    #[test]
    fn all_media_directions_serde_roundtrip() {
        let directions = [
            MediaDirection::SendRecv,
            MediaDirection::SendOnly,
            MediaDirection::RecvOnly,
            MediaDirection::Inactive,
        ];
        for dir in directions {
            let json = serde_json::to_string(&dir).unwrap();
            let parsed: MediaDirection = serde_json::from_str(&json).unwrap();
            assert_eq!(dir, parsed);
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn session_id_roundtrip_serde(val: u64) {
            let id = SessionId(val);
            let json = serde_json::to_string(&id).unwrap();
            let parsed: SessionId = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(id, parsed);
        }

        #[test]
        fn handle_id_roundtrip_serde(val: u64) {
            let id = HandleId(val);
            let json = serde_json::to_string(&id).unwrap();
            let parsed: HandleId = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(id, parsed);
        }

        #[test]
        fn rtp_payload_type_always_7bit(pt in 0u8..128) {
            let mut buf = vec![0u8; 12];
            buf[1] = pt;
            let pkt = RtpPacket::new(false, buf);
            prop_assert!(pkt.payload_type().unwrap() < 128);
        }

        #[test]
        fn rtp_sequence_number_roundtrip(seq in 0u16..=u16::MAX) {
            let mut buf = vec![0u8; 12];
            buf[2..4].copy_from_slice(&seq.to_be_bytes());
            let pkt = RtpPacket::new(false, buf);
            prop_assert_eq!(pkt.sequence_number(), Some(seq));
        }

        #[test]
        fn rtp_ssrc_roundtrip(ssrc: u32) {
            let mut buf = vec![0u8; 12];
            buf[8..12].copy_from_slice(&ssrc.to_be_bytes());
            let pkt = RtpPacket::new(false, buf);
            prop_assert_eq!(pkt.ssrc(), Some(ssrc));
        }
    }
}
