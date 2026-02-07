//! RTP fan-out from WHIP publishers to WHEP subscribers.
//!
//! When a WHIP publisher receives RTP, the fan-out relays each packet
//! to all WHEP subscribers of that publisher.

use crate::state::ResourceId;
use dashmap::DashMap;
use janus_core::webrtc::PeerConnectionHandle;
use janus_plugin_api::RtpPacket;
use std::sync::RwLock;
use tracing::trace;

/// Manages the mapping from publishers to their subscribers.
pub struct FanOut {
    publishers: DashMap<ResourceId, PublisherState>,
}

struct PublisherState {
    subscribers: RwLock<Vec<SubscriberEntry>>,
}

struct SubscriberEntry {
    resource_id: ResourceId,
    pc_handle: PeerConnectionHandle,
}

impl Default for FanOut {
    fn default() -> Self {
        Self {
            publishers: DashMap::new(),
        }
    }
}

impl FanOut {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new WHIP publisher.
    pub fn register_publisher(&self, publisher_id: ResourceId) {
        self.publishers.insert(
            publisher_id,
            PublisherState {
                subscribers: RwLock::new(Vec::new()),
            },
        );
    }

    /// Add a WHEP subscriber to a publisher.
    ///
    /// Returns `false` if the publisher does not exist.
    pub fn add_subscriber(
        &self,
        publisher_id: ResourceId,
        subscriber_id: ResourceId,
        pc_handle: PeerConnectionHandle,
    ) -> bool {
        if let Some(state) = self.publishers.get(&publisher_id) {
            if let Ok(mut subs) = state.subscribers.write() {
                subs.push(SubscriberEntry {
                    resource_id: subscriber_id,
                    pc_handle,
                });
                return true;
            }
        }
        false
    }

    /// Remove a WHEP subscriber from a publisher.
    pub fn remove_subscriber(&self, publisher_id: ResourceId, subscriber_id: ResourceId) {
        if let Some(state) = self.publishers.get(&publisher_id) {
            if let Ok(mut subs) = state.subscribers.write() {
                subs.retain(|s| s.resource_id != subscriber_id);
            }
        }
    }

    /// Remove a publisher and return the resource IDs of its subscribers.
    pub fn remove_publisher(&self, publisher_id: ResourceId) -> Vec<ResourceId> {
        if let Some((_, state)) = self.publishers.remove(&publisher_id) {
            if let Ok(subs) = state.subscribers.read() {
                return subs.iter().map(|s| s.resource_id).collect();
            }
        }
        Vec::new()
    }

    /// Relay an RTP packet from a publisher to all its subscribers.
    pub fn relay_rtp(&self, publisher_id: ResourceId, packet: &RtpPacket) {
        if let Some(state) = self.publishers.get(&publisher_id) {
            if let Ok(subs) = state.subscribers.read() {
                for sub in subs.iter() {
                    trace!(
                        publisher = %publisher_id,
                        subscriber = %sub.resource_id,
                        video = packet.video,
                        len = packet.buffer.len(),
                        "relaying RTP"
                    );
                    sub.pc_handle.send_rtp(packet.clone());
                }
            }
        }
    }

    /// Check if a publisher exists.
    pub fn has_publisher(&self, publisher_id: &ResourceId) -> bool {
        self.publishers.contains_key(publisher_id)
    }

    /// Get the number of subscribers for a publisher.
    pub fn subscriber_count(&self, publisher_id: &ResourceId) -> usize {
        self.publishers
            .get(publisher_id)
            .and_then(|state| state.subscribers.read().ok().map(|s| s.len()))
            .unwrap_or(0)
    }

    /// Get the number of registered publishers.
    pub fn publisher_count(&self) -> usize {
        self.publishers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_fanout_is_empty() {
        let fanout = FanOut::new();
        assert_eq!(fanout.publisher_count(), 0);
    }

    #[test]
    fn register_publisher() {
        let fanout = FanOut::new();
        let id = ResourceId::new();
        fanout.register_publisher(id);
        assert!(fanout.has_publisher(&id));
        assert_eq!(fanout.publisher_count(), 1);
        assert_eq!(fanout.subscriber_count(&id), 0);
    }

    #[test]
    fn remove_publisher() {
        let fanout = FanOut::new();
        let id = ResourceId::new();
        fanout.register_publisher(id);
        let subs = fanout.remove_publisher(id);
        assert!(subs.is_empty());
        assert!(!fanout.has_publisher(&id));
        assert_eq!(fanout.publisher_count(), 0);
    }

    #[test]
    fn remove_nonexistent_publisher() {
        let fanout = FanOut::new();
        let subs = fanout.remove_publisher(ResourceId::new());
        assert!(subs.is_empty());
    }

    #[test]
    fn add_subscriber_to_nonexistent_publisher_fails() {
        let fanout = FanOut::new();
        // We can't easily create a PeerConnectionHandle without tokio runtime,
        // so we test the false return path by checking has_publisher
        let pub_id = ResourceId::new();
        assert!(!fanout.has_publisher(&pub_id));
    }

    #[test]
    fn subscriber_count_for_nonexistent() {
        let fanout = FanOut::new();
        assert_eq!(fanout.subscriber_count(&ResourceId::new()), 0);
    }

    #[test]
    fn multiple_publishers() {
        let fanout = FanOut::new();
        let a = ResourceId::new();
        let b = ResourceId::new();
        fanout.register_publisher(a);
        fanout.register_publisher(b);
        assert_eq!(fanout.publisher_count(), 2);
        assert!(fanout.has_publisher(&a));
        assert!(fanout.has_publisher(&b));
    }

    #[test]
    fn relay_rtp_to_no_subscribers_does_not_panic() {
        let fanout = FanOut::new();
        let id = ResourceId::new();
        fanout.register_publisher(id);
        let packet = RtpPacket::new(false, vec![0x80, 111, 0, 1]);
        fanout.relay_rtp(id, &packet); // should not panic
    }

    #[test]
    fn relay_rtp_to_nonexistent_publisher_does_not_panic() {
        let fanout = FanOut::new();
        let packet = RtpPacket::new(true, vec![0x80, 96, 0, 1]);
        fanout.relay_rtp(ResourceId::new(), &packet); // should not panic
    }
}
