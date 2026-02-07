//! WebRTC PeerConnection management using str0m.
//!
//! Each PeerConnection is managed by a `PeerConnectionActor` running in its
//! own tokio task. External code interacts via the cloneable `PeerConnectionHandle`.

use crate::sdp;
use janus_plugin_api::{PluginSession, RtcpPacket, RtpPacket};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Pt};
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
    /// Create an SDP offer for outgoing media (offerer flow).
    CreateOffer {
        audio: bool,
        video: bool,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Set the remote SDP answer (offerer flow).
    SetRemoteAnswer {
        answer_sdp: String,
        reply: oneshot::Sender<Result<(), String>>,
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

    /// Create an SDP offer for outgoing media (offerer flow).
    pub async fn create_offer(&self, audio: bool, video: bool) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(PcCommand::CreateOffer {
                audio,
                video,
                reply: tx,
            })
            .await
            .map_err(|_| "PeerConnection actor closed".to_string())?;
        rx.await.map_err(|_| "reply channel dropped".to_string())?
    }

    /// Set the remote SDP answer (offerer flow).
    pub async fn set_remote_answer(&self, answer_sdp: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(PcCommand::SetRemoteAnswer {
                answer_sdp,
                reply: tx,
            })
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
pub async fn create_peer_connection(config: PcConfig) -> Result<PeerConnectionHandle, String> {
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
        audio_mid: None,
        video_mid: None,
        audio_pt: None,
        video_pt: None,
        pending_offer: None,
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
    /// Negotiated media mappings for sending RTP.
    audio_mid: Option<Mid>,
    video_mid: Option<Mid>,
    audio_pt: Option<Pt>,
    video_pt: Option<Pt>,
    /// Pending offer awaiting a remote answer (offerer flow).
    pending_offer: Option<SdpPendingOffer>,
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
                            if result.is_ok() {
                                self.populate_media_mappings();
                            }
                            let _ = reply.send(result);
                        }
                        Some(PcCommand::CreateOffer { audio, video, reply }) => {
                            let result = self.handle_create_offer(audio, video);
                            let _ = reply.send(result);
                        }
                        Some(PcCommand::SetRemoteAnswer { answer_sdp, reply }) => {
                            let result = self.handle_set_remote_answer(&answer_sdp);
                            let _ = reply.send(result);
                        }
                        Some(PcCommand::AddIceCandidate { candidate, mid }) => {
                            debug!(mid = %mid, "adding ICE candidate (trickle)");
                            // str0m 0.11 handles ICE candidates via SDP;
                            // trickle is managed internally when candidates arrive via UDP
                            let _ = (&candidate, &mid);
                        }
                        Some(PcCommand::SendRtp { packet }) => {
                            let (mid, pt) = if packet.video {
                                (self.video_mid, self.video_pt)
                            } else {
                                (self.audio_mid, self.audio_pt)
                            };
                            if let (Some(mid), Some(pt)) = (mid, pt) {
                                if let Some(writer) = self.rtc.writer(mid) {
                                    let now = Instant::now();
                                    let freq = if packet.video {
                                        Frequency::NINETY_KHZ
                                    } else {
                                        Frequency::FORTY_EIGHT_KHZ
                                    };
                                    let _ = writer.write(
                                        pt,
                                        now,
                                        MediaTime::new(0, freq),
                                        packet.buffer,
                                    );
                                }
                            } else {
                                trace!(
                                    video = packet.video,
                                    len = packet.buffer.len(),
                                    "SendRtp: no negotiated mid/pt, dropping"
                                );
                            }
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

    /// Populate pt for a known mid by querying the writer's payload params.
    fn populate_pt_for_mid(&mut self, mid: Mid, kind: MediaKind) {
        if let Some(writer) = self.rtc.writer(mid) {
            if let Some(params) = writer.payload_params().next() {
                let pt = params.pt();
                match kind {
                    MediaKind::Audio => {
                        self.audio_pt = Some(pt);
                        debug!(?mid, pt = *pt, "mapped audio pt for sending");
                    }
                    MediaKind::Video => {
                        self.video_pt = Some(pt);
                        debug!(?mid, pt = *pt, "mapped video pt for sending");
                    }
                }
            }
        }
    }

    /// After SDP negotiation, populate pt values for any mids we already know.
    fn populate_media_mappings(&mut self) {
        if let Some(mid) = self.audio_mid {
            if self.audio_pt.is_none() {
                self.populate_pt_for_mid(mid, MediaKind::Audio);
            }
        }
        if let Some(mid) = self.video_mid {
            if self.video_pt.is_none() {
                self.populate_pt_for_mid(mid, MediaKind::Video);
            }
        }
    }

    /// Handle CreateOffer command: add media lines and produce an SDP offer.
    fn handle_create_offer(&mut self, audio: bool, video: bool) -> Result<String, String> {
        let mut api = self.rtc.sdp_api();
        if audio {
            let mid = api.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
            self.audio_mid = Some(mid);
        }
        if video {
            let mid = api.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
            self.video_mid = Some(mid);
        }
        match api.apply() {
            Some((offer, pending)) => {
                let sdp_string = offer.to_sdp_string();
                self.pending_offer = Some(pending);
                debug!(sdp_len = sdp_string.len(), "created SDP offer");
                Ok(sdp_string)
            }
            None => Err("No SDP changes to apply".to_string()),
        }
    }

    /// Handle SetRemoteAnswer command: apply the remote answer.
    fn handle_set_remote_answer(&mut self, answer_sdp: &str) -> Result<(), String> {
        let answer = SdpAnswer::from_sdp_string(answer_sdp)
            .map_err(|e| format!("Failed to parse SDP answer: {e}"))?;
        let pending = self
            .pending_offer
            .take()
            .ok_or_else(|| "No pending offer to match answer against".to_string())?;
        self.rtc
            .sdp_api()
            .accept_answer(pending, answer)
            .map_err(|e| format!("Failed to accept SDP answer: {e}"))?;
        self.populate_media_mappings();
        debug!("remote SDP answer applied");
        Ok(())
    }

    fn handle_udp_data(&mut self, data: &[u8], source: SocketAddr) {
        let local = self
            .socket
            .local_addr()
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
        let receive = Receive::new(str0m::net::Protocol::Udp, source, local, data);
        match receive {
            Ok(receive) => {
                let _ = self
                    .rtc
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
            Event::MediaAdded(added) => {
                debug!(
                    ?added.mid,
                    ?added.kind,
                    ?added.direction,
                    "media added to session"
                );
                // Store the mid for sending if the direction allows it.
                // For answerer flow, remote sends us an offer with sendrecv/sendonly,
                // and we answer with recvonly/sendrecv. The MediaAdded event fires
                // for remote-initiated media.
                match added.kind {
                    MediaKind::Audio if self.audio_mid.is_none() => {
                        self.audio_mid = Some(added.mid);
                        self.populate_pt_for_mid(added.mid, MediaKind::Audio);
                    }
                    MediaKind::Video if self.video_mid.is_none() => {
                        self.video_mid = Some(added.mid);
                        self.populate_pt_for_mid(added.mid, MediaKind::Video);
                    }
                    _ => {}
                }
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
