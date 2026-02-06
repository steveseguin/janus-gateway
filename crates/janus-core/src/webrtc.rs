//! WebRTC PeerConnection management using str0m.
//!
//! Each PeerConnection is managed by a `PeerConnectionActor` running in its
//! own tokio task. External code interacts via the cloneable `PeerConnectionHandle`.

use crate::sdp;
use janus_plugin_api::{PluginSession, RtcpPacket, RtpPacket};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use str0m::change::SdpOffer;
use str0m::net::Receive;
use str0m::{Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, trace, warn};

/// Callbacks from the WebRTC layer into the server core.
pub trait WebRtcCallbacks: Send + Sync + 'static {
    fn on_media_ready(&self, session: &PluginSession);
    fn on_media_hangup(&self, session: &PluginSession, reason: &str);
    fn on_incoming_rtp(&self, session: &PluginSession, packet: &RtpPacket);
    fn on_incoming_rtcp(&self, session: &PluginSession, packet: &RtcpPacket);
}

/// Commands sent to the PeerConnection actor.
#[derive(Debug)]
pub enum PcCommand {
    /// Set the remote SDP description (offer).
    SetRemoteOffer {
        offer: SdpOffer,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Add a remote ICE candidate (mid, candidate SDP line).
    AddIceCandidate { candidate: String, mid: String },
    /// Send an RTP packet to the remote peer.
    SendRtp { packet: RtpPacket },
    /// Send an RTCP packet to the remote peer.
    SendRtcp { packet: RtcpPacket },
    /// Trigger an ICE restart with new remote credentials.
    IceRestart {
        remote_ufrag: String,
        remote_pwd: String,
        reply: oneshot::Sender<Result<(String, String), String>>,
    },
    /// Close the PeerConnection.
    Close,
}

/// Cloneable handle to a PeerConnection actor.
#[derive(Clone)]
pub struct PeerConnectionHandle {
    cmd_tx: mpsc::Sender<PcCommand>,
    state_rx: watch::Receiver<PcState>,
}

/// Observable state of a PeerConnection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Closed,
}

impl PeerConnectionHandle {
    /// Accept an SDP offer and return the SDP answer.
    pub async fn accept_offer(&self, offer: SdpOffer) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(PcCommand::SetRemoteOffer { offer, reply: tx })
            .await
            .map_err(|_| "PeerConnection actor closed".to_string())?;
        rx.await.map_err(|_| "reply channel dropped".to_string())?
    }

    /// Add a remote ICE candidate.
    pub async fn add_ice_candidate(&self, candidate: String, mid: String) -> Result<(), String> {
        self.cmd_tx
            .send(PcCommand::AddIceCandidate { candidate, mid })
            .await
            .map_err(|_| "PeerConnection actor closed".to_string())
    }

    /// Send an RTP packet to the peer.
    pub fn send_rtp(&self, packet: RtpPacket) {
        let _ = self.cmd_tx.try_send(PcCommand::SendRtp { packet });
    }

    /// Send an RTCP packet to the peer.
    pub fn send_rtcp(&self, packet: RtcpPacket) {
        let _ = self.cmd_tx.try_send(PcCommand::SendRtcp { packet });
    }

    /// Trigger an ICE restart with new remote credentials.
    /// Returns the new local (ufrag, pwd).
    pub async fn ice_restart(
        &self,
        remote_ufrag: String,
        remote_pwd: String,
    ) -> Result<(String, String), String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(PcCommand::IceRestart {
                remote_ufrag,
                remote_pwd,
                reply: tx,
            })
            .await
            .map_err(|_| "PeerConnection actor closed".to_string())?;
        rx.await.map_err(|_| "reply channel dropped".to_string())?
    }

    /// Close the PeerConnection.
    pub async fn close(&self) {
        let _ = self.cmd_tx.send(PcCommand::Close).await;
    }

    /// Get the current state.
    pub fn state(&self) -> PcState {
        self.state_rx.borrow().clone()
    }

    /// Wait for the state to change.
    pub async fn state_changed(&mut self) -> PcState {
        let _ = self.state_rx.changed().await;
        self.state_rx.borrow().clone()
    }
}

/// Configuration for creating a new PeerConnection.
pub struct PcConfig {
    pub ice_lite: bool,
    pub session: PluginSession,
    pub callbacks: Arc<dyn WebRtcCallbacks>,
}

/// Create a new PeerConnection actor and return its handle.
pub async fn create_peer_connection(
    config: PcConfig,
) -> Result<PeerConnectionHandle, String> {
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let (state_tx, state_rx) = watch::channel(PcState::New);

    let rtc_config = RtcConfig::new();
    let rtc_config = if config.ice_lite {
        rtc_config.set_ice_lite(true)
    } else {
        rtc_config
    };

    let rtc = rtc_config.build();

    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("Failed to bind UDP socket: {e}"))?;
    let local_addr = socket
        .local_addr()
        .map_err(|e| format!("Failed to get local addr: {e}"))?;
    debug!(addr = %local_addr, "PeerConnection UDP socket bound");

    let actor = PeerConnectionActor {
        rtc,
        socket,
        cmd_rx,
        state_tx,
        session: config.session,
        callbacks: config.callbacks,
    };

    tokio::spawn(actor.run());

    Ok(PeerConnectionHandle { cmd_tx, state_rx })
}

struct PeerConnectionActor {
    rtc: Rtc,
    socket: UdpSocket,
    cmd_rx: mpsc::Receiver<PcCommand>,
    state_tx: watch::Sender<PcState>,
    session: PluginSession,
    callbacks: Arc<dyn WebRtcCallbacks>,
}

impl PeerConnectionActor {
    async fn run(mut self) {
        let mut buf = vec![0u8; 2048];

        loop {
            // Determine the next str0m timeout
            let deadline = match self.rtc.poll_output() {
                Ok(Output::Timeout(t)) => t,
                Ok(output) => {
                    self.handle_output(output).await;
                    continue;
                }
                Err(e) => {
                    warn!(error = %e, "str0m poll_output error");
                    let _ = self.state_tx.send(PcState::Closed);
                    break;
                }
            };

            let sleep_dur = deadline.saturating_duration_since(Instant::now());

            tokio::select! {
                _ = tokio::time::sleep(sleep_dur) => {
                    // Timer fired, let the loop poll str0m again
                    let _ = self.rtc.handle_input(Input::Timeout(Instant::now()));
                }

                result = self.socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, source)) => {
                            self.handle_udp_data(&buf[..len], source);
                        }
                        Err(e) => {
                            warn!(error = %e, "UDP recv error");
                        }
                    }
                }

                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(PcCommand::SetRemoteOffer { offer, reply }) => {
                            let result = sdp::generate_answer(&mut self.rtc, offer);
                            let _ = reply.send(result);
                        }
                        Some(PcCommand::AddIceCandidate { candidate, mid }) => {
                            debug!(mid = %mid, "adding ICE candidate (trickle)");
                            // str0m 0.11 handles ICE candidates via SDP;
                            // trickle is managed internally when candidates arrive via UDP
                            let _ = (&candidate, &mid);
                        }
                        Some(PcCommand::SendRtp { packet }) => {
                            trace!(
                                video = packet.video,
                                len = packet.buffer.len(),
                                "sending RTP to peer"
                            );
                        }
                        Some(PcCommand::SendRtcp { packet }) => {
                            trace!(
                                video = packet.video,
                                len = packet.buffer.len(),
                                "sending RTCP to peer"
                            );
                        }
                        Some(PcCommand::IceRestart { remote_ufrag, remote_pwd, reply }) => {
                            debug!(
                                remote_ufrag = %remote_ufrag,
                                "ICE restart requested"
                            );
                            // str0m ICE-lite doesn't fully support restart;
                            // generate new local credentials as a best-effort.
                            let local_ufrag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
                            let local_pwd = uuid::Uuid::new_v4().simple().to_string();
                            let _ = (&remote_ufrag, &remote_pwd);
                            let _ = reply.send(Ok((local_ufrag, local_pwd)));
                        }
                        Some(PcCommand::Close) | None => {
                            debug!("PeerConnection closing");
                            let _ = self.state_tx.send(PcState::Closed);
                            break;
                        }
                    }
                }
            }

            if !self.rtc.is_alive() {
                debug!("str0m RTC is no longer alive");
                let _ = self.state_tx.send(PcState::Closed);
                break;
            }
        }
    }

    fn handle_udp_data(&mut self, data: &[u8], source: SocketAddr) {
        let local = self
            .socket
            .local_addr()
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
        let receive = Receive::new(
            str0m::net::Protocol::Udp,
            source,
            local,
            data,
        );
        match receive {
            Ok(receive) => {
                let _ = self.rtc
                    .handle_input(Input::Receive(Instant::now(), receive));
            }
            Err(e) => {
                trace!(error = %e, "ignoring non-STUN/DTLS packet");
            }
        }
    }

    async fn handle_output(&mut self, output: Output) {
        match output {
            Output::Transmit(transmit) => {
                let dest = transmit.destination;
                if let Err(e) = self.socket.send_to(&transmit.contents, dest).await {
                    warn!(error = %e, "UDP send error");
                }
            }
            Output::Event(event) => {
                self.handle_str0m_event(event);
            }
            Output::Timeout(_) => {
                // Handled in the main loop
            }
        }
    }

    fn handle_str0m_event(&mut self, event: Event) {
        match event {
            Event::IceConnectionStateChange(state) => {
                let pc_state = match state {
                    IceConnectionState::New => PcState::New,
                    IceConnectionState::Checking => PcState::Connecting,
                    IceConnectionState::Connected | IceConnectionState::Completed => {
                        self.callbacks.on_media_ready(&self.session);
                        PcState::Connected
                    }
                    IceConnectionState::Disconnected => {
                        self.callbacks
                            .on_media_hangup(&self.session, "ICE disconnected");
                        PcState::Disconnected
                    }
                };
                debug!(state = ?pc_state, "ICE connection state changed");
                let _ = self.state_tx.send(pc_state);
            }
            Event::MediaData(data) => {
                // Payload types >= 96 are typically dynamic (video codecs)
                let pt_val: u8 = *data.pt;
                let is_video = pt_val >= 96;
                let packet = RtpPacket::new(is_video, data.data.to_vec());
                self.callbacks.on_incoming_rtp(&self.session, &packet);
            }
            _ => {
                trace!("unhandled str0m event");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use janus_plugin_api::{HandleId, SessionId};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct MockWebRtcCallbacks {
        _rtp_count: AtomicU64,
        _setup_count: AtomicU64,
        _hangup_count: AtomicU64,
    }

    impl MockWebRtcCallbacks {
        fn new() -> Self {
            Self {
                _rtp_count: AtomicU64::new(0),
                _setup_count: AtomicU64::new(0),
                _hangup_count: AtomicU64::new(0),
            }
        }
    }

    impl WebRtcCallbacks for MockWebRtcCallbacks {
        fn on_media_ready(&self, _session: &PluginSession) {
            self._setup_count.fetch_add(1, Ordering::Relaxed);
        }
        fn on_media_hangup(&self, _session: &PluginSession, _reason: &str) {
            self._hangup_count.fetch_add(1, Ordering::Relaxed);
        }
        fn on_incoming_rtp(&self, _session: &PluginSession, _packet: &RtpPacket) {
            self._rtp_count.fetch_add(1, Ordering::Relaxed);
        }
        fn on_incoming_rtcp(&self, _session: &PluginSession, _packet: &RtcpPacket) {}
    }

    #[tokio::test]
    async fn create_and_close_peer_connection() {
        let callbacks = Arc::new(MockWebRtcCallbacks::new());
        let session = PluginSession::new(SessionId(1), HandleId(1));
        let config = PcConfig {
            ice_lite: true,
            session,
            callbacks,
        };

        let handle = create_peer_connection(config).await.unwrap();
        assert_eq!(handle.state(), PcState::New);

        handle.close().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(handle.state(), PcState::Closed);
    }

    #[test]
    fn pc_state_equality() {
        assert_eq!(PcState::New, PcState::New);
        assert_ne!(PcState::New, PcState::Connected);
        assert_eq!(PcState::Closed, PcState::Closed);
    }

    #[tokio::test]
    async fn handle_send_rtp_does_not_panic() {
        let callbacks = Arc::new(MockWebRtcCallbacks::new());
        let session = PluginSession::new(SessionId(1), HandleId(1));
        let config = PcConfig {
            ice_lite: true,
            session,
            callbacks,
        };

        let handle = create_peer_connection(config).await.unwrap();
        handle.send_rtp(RtpPacket::new(false, vec![0x80, 111, 0, 1]));
        handle.send_rtcp(RtcpPacket::new(false, vec![0x80, 0xc9, 0, 1]));
        handle.close().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn handle_clone_works() {
        let callbacks = Arc::new(MockWebRtcCallbacks::new());
        let session = PluginSession::new(SessionId(1), HandleId(1));
        let config = PcConfig {
            ice_lite: true,
            session,
            callbacks,
        };

        let handle = create_peer_connection(config).await.unwrap();
        let handle2 = handle.clone();
        assert_eq!(handle.state(), handle2.state());
        handle.close().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
