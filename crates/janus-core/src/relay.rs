//! RTP/RTCP relay engine.
//!
//! Routes incoming media packets from WebRTC peers to their attached plugin,
//! and relays outgoing packets from plugins back to the peer.

use janus_plugin_api::{HandleId, RtcpPacket, RtpPacket, SessionId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

/// Commands the relay processes.
#[derive(Debug)]
pub enum RelayCommand {
    /// An RTP packet arrived from a peer.
    IncomingRtp {
        session_id: SessionId,
        handle_id: HandleId,
        packet: RtpPacket,
    },
    /// An RTCP packet arrived from a peer.
    IncomingRtcp {
        session_id: SessionId,
        handle_id: HandleId,
        packet: RtcpPacket,
    },
    /// A plugin wants to send an RTP packet to the peer.
    OutgoingRtp {
        session_id: SessionId,
        handle_id: HandleId,
        packet: RtpPacket,
    },
    /// A plugin wants to send an RTCP packet to the peer.
    OutgoingRtcp {
        session_id: SessionId,
        handle_id: HandleId,
        packet: RtcpPacket,
    },
}

/// Sink for outgoing media that the WebRTC layer provides.
#[async_trait::async_trait]
pub trait MediaSink: Send + Sync + 'static {
    /// Send an RTP packet to the peer.
    async fn send_rtp(&self, session_id: SessionId, handle_id: HandleId, packet: &RtpPacket);
    /// Send an RTCP packet to the peer.
    async fn send_rtcp(&self, session_id: SessionId, handle_id: HandleId, packet: &RtcpPacket);
}

/// Packet counters for a single handle.
#[derive(Debug, Default, Clone)]
pub struct RelayStats {
    pub rtp_in: u64,
    pub rtp_out: u64,
    pub rtcp_in: u64,
    pub rtcp_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// Per-handle relay state.
#[derive(Debug)]
struct HandleRelay {
    session_id: SessionId,
    handle_id: HandleId,
    stats: RelayStats,
}

/// The relay engine processes media commands from a channel.
pub struct RelayEngine {
    handles: HashMap<(SessionId, HandleId), HandleRelay>,
    command_rx: mpsc::Receiver<RelayCommand>,
}

/// A sender handle that can be cloned and used to submit commands.
#[derive(Clone)]
pub struct RelaySender {
    tx: mpsc::Sender<RelayCommand>,
}

impl RelaySender {
    /// Submit a relay command. Returns false if the channel is closed.
    pub async fn send(&self, cmd: RelayCommand) -> bool {
        self.tx.send(cmd).await.is_ok()
    }

    /// Try to submit without blocking. Returns false if full or closed.
    pub fn try_send(&self, cmd: RelayCommand) -> bool {
        self.tx.try_send(cmd).is_ok()
    }
}

/// Create a relay engine and its command sender.
pub fn create_relay(buffer_size: usize) -> (RelayEngine, RelaySender) {
    let (tx, rx) = mpsc::channel(buffer_size);
    let engine = RelayEngine {
        handles: HashMap::new(),
        command_rx: rx,
    };
    let sender = RelaySender { tx };
    (engine, sender)
}

impl RelayEngine {
    /// Register a handle for relay tracking.
    pub fn register_handle(&mut self, session_id: SessionId, handle_id: HandleId) {
        self.handles.insert(
            (session_id, handle_id),
            HandleRelay {
                session_id,
                handle_id,
                stats: RelayStats::default(),
            },
        );
        debug!(session_id = %session_id, handle_id = %handle_id, "relay handle registered");
    }

    /// Unregister a handle.
    pub fn unregister_handle(&mut self, session_id: SessionId, handle_id: HandleId) {
        self.handles.remove(&(session_id, handle_id));
        debug!(session_id = %session_id, handle_id = %handle_id, "relay handle unregistered");
    }

    /// Get stats for a handle.
    pub fn stats(&self, session_id: SessionId, handle_id: HandleId) -> Option<&RelayStats> {
        self.handles
            .get(&(session_id, handle_id))
            .map(|h| &h.stats)
    }

    /// Process a single command, updating stats. Returns the command for the
    /// caller to dispatch to the appropriate plugin or WebRTC sink.
    pub fn process_command(&mut self, cmd: RelayCommand) -> RelayCommand {
        match &cmd {
            RelayCommand::IncomingRtp {
                session_id,
                handle_id,
                packet,
            } => {
                if let Some(h) = self.handles.get_mut(&(*session_id, *handle_id)) {
                    h.stats.rtp_in += 1;
                    h.stats.bytes_in += packet.buffer.len() as u64;
                }
                trace!(
                    session_id = %session_id,
                    handle_id = %handle_id,
                    video = packet.video,
                    len = packet.buffer.len(),
                    "incoming RTP"
                );
            }
            RelayCommand::IncomingRtcp {
                session_id,
                handle_id,
                ..
            } => {
                if let Some(h) = self.handles.get_mut(&(*session_id, *handle_id)) {
                    h.stats.rtcp_in += 1;
                }
            }
            RelayCommand::OutgoingRtp {
                session_id,
                handle_id,
                packet,
            } => {
                if let Some(h) = self.handles.get_mut(&(*session_id, *handle_id)) {
                    h.stats.rtp_out += 1;
                    h.stats.bytes_out += packet.buffer.len() as u64;
                }
                trace!(
                    session_id = %session_id,
                    handle_id = %handle_id,
                    video = packet.video,
                    len = packet.buffer.len(),
                    "outgoing RTP"
                );
            }
            RelayCommand::OutgoingRtcp {
                session_id,
                handle_id,
                ..
            } => {
                if let Some(h) = self.handles.get_mut(&(*session_id, *handle_id)) {
                    h.stats.rtcp_out += 1;
                }
            }
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rtp(video: bool, len: usize) -> RtpPacket {
        RtpPacket::new(video, vec![0u8; len])
    }

    fn make_rtcp(video: bool) -> RtcpPacket {
        RtcpPacket::new(video, vec![0u8; 32])
    }

    #[test]
    fn create_relay_returns_engine_and_sender() {
        let (engine, _sender) = create_relay(100);
        assert!(engine.handles.is_empty());
    }

    #[test]
    fn register_and_unregister_handle() {
        let (mut engine, _) = create_relay(100);
        let sid = SessionId(1);
        let hid = HandleId(2);

        engine.register_handle(sid, hid);
        assert!(engine.stats(sid, hid).is_some());

        engine.unregister_handle(sid, hid);
        assert!(engine.stats(sid, hid).is_none());
    }

    #[test]
    fn process_incoming_rtp_updates_stats() {
        let (mut engine, _) = create_relay(100);
        let sid = SessionId(1);
        let hid = HandleId(2);
        engine.register_handle(sid, hid);

        engine.process_command(RelayCommand::IncomingRtp {
            session_id: sid,
            handle_id: hid,
            packet: make_rtp(false, 160),
        });

        let stats = engine.stats(sid, hid).unwrap();
        assert_eq!(stats.rtp_in, 1);
        assert_eq!(stats.bytes_in, 160);
        assert_eq!(stats.rtp_out, 0);
    }

    #[test]
    fn process_outgoing_rtp_updates_stats() {
        let (mut engine, _) = create_relay(100);
        let sid = SessionId(1);
        let hid = HandleId(2);
        engine.register_handle(sid, hid);

        engine.process_command(RelayCommand::OutgoingRtp {
            session_id: sid,
            handle_id: hid,
            packet: make_rtp(true, 1200),
        });

        let stats = engine.stats(sid, hid).unwrap();
        assert_eq!(stats.rtp_out, 1);
        assert_eq!(stats.bytes_out, 1200);
    }

    #[test]
    fn process_rtcp_updates_stats() {
        let (mut engine, _) = create_relay(100);
        let sid = SessionId(1);
        let hid = HandleId(2);
        engine.register_handle(sid, hid);

        engine.process_command(RelayCommand::IncomingRtcp {
            session_id: sid,
            handle_id: hid,
            packet: make_rtcp(false),
        });
        engine.process_command(RelayCommand::OutgoingRtcp {
            session_id: sid,
            handle_id: hid,
            packet: make_rtcp(true),
        });

        let stats = engine.stats(sid, hid).unwrap();
        assert_eq!(stats.rtcp_in, 1);
        assert_eq!(stats.rtcp_out, 1);
    }

    #[test]
    fn process_command_for_unknown_handle_no_panic() {
        let (mut engine, _) = create_relay(100);
        // Should not panic even though handle isn't registered.
        engine.process_command(RelayCommand::IncomingRtp {
            session_id: SessionId(999),
            handle_id: HandleId(888),
            packet: make_rtp(false, 100),
        });
    }

    #[test]
    fn multiple_packets_accumulate_stats() {
        let (mut engine, _) = create_relay(100);
        let sid = SessionId(1);
        let hid = HandleId(1);
        engine.register_handle(sid, hid);

        for i in 0..100 {
            engine.process_command(RelayCommand::IncomingRtp {
                session_id: sid,
                handle_id: hid,
                packet: make_rtp(false, 160),
            });
        }

        let stats = engine.stats(sid, hid).unwrap();
        assert_eq!(stats.rtp_in, 100);
        assert_eq!(stats.bytes_in, 16000);
    }

    #[tokio::test]
    async fn sender_can_send_commands() {
        let (mut engine, sender) = create_relay(100);
        let sid = SessionId(1);
        let hid = HandleId(1);
        engine.register_handle(sid, hid);

        assert!(
            sender
                .send(RelayCommand::IncomingRtp {
                    session_id: sid,
                    handle_id: hid,
                    packet: make_rtp(false, 160),
                })
                .await
        );
    }

    #[test]
    fn try_send_on_closed_channel_returns_false() {
        let (engine, sender) = create_relay(1);
        drop(engine); // drop the receiver
        assert!(!sender.try_send(RelayCommand::IncomingRtp {
            session_id: SessionId(1),
            handle_id: HandleId(1),
            packet: make_rtp(false, 10),
        }));
    }
}
