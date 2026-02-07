# Janus Gateway — Rust Rewrite Plan

## 1. Executive Summary

Janus Gateway is a ~121K-line C WebRTC server with a plugin architecture.
This plan proposes a phased rewrite into idiomatic Rust, preserving the
modular plugin/transport/event-handler architecture while gaining memory
safety, fearless concurrency, and a modern testing story.

**Key decisions up front:**

| Decision | Choice | Rationale |
|----------|--------|-----------|
| WebRTC stack | **str0m** (sans-I/O) | Purpose-built for SFU/MCU servers; no hidden threads; full control over I/O |
| Async runtime | **tokio** | Industry standard, excellent ecosystem |
| Plugin system | Rust traits + dynamic loading (`libloading`) | Mirrors current `dlopen`/`dlsym` model |
| Config format | **TOML** (with `.jcfg` compat shim) | Idiomatic Rust; serde support; similar feel to libconfig |
| Signaling/API | **axum** (HTTP) + **tokio-tungstenite** (WS) | Fast, composable, async-native |
| SIP | **rsip** + **sofia-sip-sys** FFI bridge | Pure Rust SIP parsing; FFI to sofia-sip for UA stack until pure replacement matures |
| Testing | **cargo test** + **proptest** + integration harness | Unit, property-based, and end-to-end |

---

## 2. Current Architecture (C)

```
┌─────────────────────────────────────────────────┐
│                   janus.c (core)                │
│         sessions · ICE · DTLS · RTP relay       │
├────────────┬──────────────┬─────────────────────┤
│  Plugins   │  Transports  │  Event Handlers     │
│  (.so)     │  (.so)       │  (.so)              │
│ videoroom  │  http        │  sample             │
│ audiobridge│  websockets  │  mqtt               │
│ sip        │  mqtt        │  websocket          │
│ streaming  │  rabbitmq    │  rabbitmq           │
│ echotest   │  pfunix      │  gelf               │
│ videocall  │  nanomsg     │  nanomsg            │
│ recordplay │              │                     │
│ textroom   │              │                     │
│ nosip      │              │                     │
│ lua        │              │                     │
│ duktape    │              │                     │
└────────────┴──────────────┴─────────────────────┘
```

### Source breakdown (~121K lines)

| Component | Files | ~Lines |
|-----------|-------|--------|
| Core (janus.c, ice.c, dtls.c, rtp.c, rtcp.c, sdp.c, …) | ~30 | ~45K |
| Plugins | 11 | ~63K |
| Transports | 6 | ~8K |
| Event handlers | 6 | ~5K |

### Key C dependencies to replace

| C Library | Purpose | Rust Replacement |
|-----------|---------|-----------------|
| libnice | ICE | str0m (built-in ICE) |
| libsrtp2 | SRTP | str0m (built-in SRTP) |
| OpenSSL / BoringSSL | DTLS, TLS | str0m (built-in DTLS) + rustls (TLS for transports) |
| libmicrohttpd | HTTP server | axum |
| libwebsockets | WebSocket server | tokio-tungstenite |
| sofia-sip | SIP UA | rsip (parsing) + sofia-sip-sys FFI (UA, phase 1) |
| GLib | data structures, threads, main loop | std + tokio + dashmap |
| Jansson | JSON | serde_json |
| libconfig | Config parsing | toml + serde |
| usrsctp | SCTP/DataChannels | str0m (built-in SCTP) |
| libopus | Opus codec | opus (crate, FFI) or audiopus |
| librnnoise | Noise suppression | rnnoise-c (FFI) |

---

## 3. Target Architecture (Rust)

```
janus-gateway-rs/
├── Cargo.toml                    # workspace root
├── crates/
│   ├── janus-core/               # core server: sessions, media relay, ICE
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── server.rs         # main server loop, signal handling
│   │   │   ├── session.rs        # session + handle management
│   │   │   ├── media.rs          # RTP/RTCP relay, recording
│   │   │   ├── webrtc.rs         # str0m integration, ICE/DTLS/SRTP
│   │   │   ├── sdp.rs            # SDP generation/manipulation
│   │   │   ├── config.rs         # TOML config with jcfg compat
│   │   │   ├── admin.rs          # admin API
│   │   │   ├── auth.rs           # token + API secret auth
│   │   │   ├── events.rs         # event bus (broadcast channel)
│   │   │   └── error.rs          # unified error types
│   │   └── tests/
│   │
│   ├── janus-plugin-api/         # trait definitions for plugins
│   │   ├── src/lib.rs            # JanusPlugin trait, callback types
│   │   └── tests/
│   │
│   ├── janus-transport-api/      # trait definitions for transports
│   │   ├── src/lib.rs            # JanusTransport trait
│   │   └── tests/
│   │
│   ├── janus-event-api/          # trait definitions for event handlers
│   │   ├── src/lib.rs            # JanusEventHandler trait
│   │   └── tests/
│   │
│   ├── plugins/
│   │   ├── janus-plugin-echotest/
│   │   ├── janus-plugin-videoroom/
│   │   ├── janus-plugin-audiobridge/
│   │   ├── janus-plugin-streaming/
│   │   ├── janus-plugin-sip/
│   │   ├── janus-plugin-nosip/
│   │   ├── janus-plugin-videocall/
│   │   ├── janus-plugin-recordplay/
│   │   ├── janus-plugin-textroom/
│   │   └── janus-plugin-lua/     # mlua for Lua scripting
│   │
│   ├── transports/
│   │   ├── janus-transport-http/       # axum
│   │   ├── janus-transport-websocket/  # tokio-tungstenite
│   │   ├── janus-transport-mqtt/       # rumqttc
│   │   ├── janus-transport-rabbitmq/   # lapin
│   │   ├── janus-transport-unix/       # tokio UDS
│   │   └── janus-transport-nanomsg/    # nng-rs
│   │
│   └── events/
│       ├── janus-event-sample/
│       ├── janus-event-mqtt/
│       ├── janus-event-websocket/
│       ├── janus-event-rabbitmq/
│       ├── janus-event-gelf/
│       └── janus-event-nanomsg/
│
├── tests/                        # integration + end-to-end tests
│   ├── common/mod.rs             # test harness (start server, connect)
│   ├── echotest_e2e.rs
│   ├── videoroom_e2e.rs
│   └── ...
│
└── benches/                      # criterion benchmarks
    ├── rtp_relay.rs
    ├── session_throughput.rs
    └── sdp_parsing.rs
```

---

## 4. Core Trait Definitions

### 4.1 Plugin API

```rust
// crates/janus-plugin-api/src/lib.rs

/// Every plugin implements this trait.
#[async_trait]
pub trait JanusPlugin: Send + Sync + 'static {
    // --- metadata ---
    fn name(&self) -> &'static str;
    fn package(&self) -> &'static str;
    fn version(&self) -> Version;
    fn description(&self) -> &'static str;

    // --- lifecycle ---
    async fn init(&mut self, callbacks: Arc<dyn PluginCallbacks>, config_path: &Path) -> Result<()>;
    async fn destroy(&mut self) -> Result<()>;

    // --- sessions ---
    async fn create_session(&self, session_id: SessionId) -> Result<()>;
    async fn destroy_session(&self, session_id: SessionId) -> Result<()>;
    fn query_session(&self, session_id: SessionId) -> Result<serde_json::Value>;

    // --- signaling ---
    async fn handle_message(
        &self,
        session_id: SessionId,
        transaction: &str,
        body: serde_json::Value,
        jsep: Option<Jsep>,
    ) -> Result<PluginResult>;

    async fn handle_admin_message(
        &self,
        _body: serde_json::Value,
    ) -> Result<serde_json::Value> {
        Err(Error::NotImplemented)
    }

    // --- media (optional, default no-op) ---
    fn setup_media(&self, _session_id: SessionId) {}
    fn hangup_media(&self, _session_id: SessionId, _reason: &str) {}
    fn incoming_rtp(&self, _session_id: SessionId, _packet: &RtpPacket) {}
    fn incoming_rtcp(&self, _session_id: SessionId, _packet: &RtcpPacket) {}
    fn incoming_data(&self, _session_id: SessionId, _label: &str, _data: &[u8]) {}
    fn data_ready(&self, _session_id: SessionId) {}
    fn slow_link(&self, _session_id: SessionId, _uplink: bool, _lost: u32) {}
}

/// Callbacks the core provides to plugins.
#[async_trait]
pub trait PluginCallbacks: Send + Sync {
    fn relay_rtp(&self, session_id: SessionId, packet: &RtpPacket);
    fn relay_rtcp(&self, session_id: SessionId, packet: &RtcpPacket);
    fn relay_data(&self, session_id: SessionId, label: &str, data: &[u8]);
    async fn push_event(
        &self,
        session_id: SessionId,
        transaction: &str,
        body: serde_json::Value,
        jsep: Option<Jsep>,
    ) -> Result<()>;
    fn close_pc(&self, session_id: SessionId);
    fn end_session(&self, session_id: SessionId);
    fn notify_event(&self, event: PluginEvent);
}
```

### 4.2 Transport API

```rust
#[async_trait]
pub trait JanusTransport: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn package(&self) -> &'static str;

    async fn init(&mut self, callbacks: Arc<dyn TransportCallbacks>, config_path: &Path) -> Result<()>;
    async fn destroy(&mut self) -> Result<()>;

    fn is_janus_api_enabled(&self) -> bool;
    fn is_admin_api_enabled(&self) -> bool;

    async fn send_message(&self, session_id: SessionId, msg: serde_json::Value) -> Result<()>;
    fn session_over(&self, session_id: SessionId);
}
```

### 4.3 Event Handler API

```rust
#[async_trait]
pub trait JanusEventHandler: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn events_mask(&self) -> EventMask;

    async fn init(&mut self, config_path: &Path) -> Result<()>;
    async fn destroy(&mut self) -> Result<()>;

    async fn handle_event(&self, event: &JanusEvent) -> Result<()>;
}
```

### 4.4 Dynamic Loading

```rust
// Plugins are compiled as cdylib crates exporting:
#[no_mangle]
pub extern "C" fn janus_plugin_create() -> *mut dyn JanusPlugin {
    let plugin = EchoTestPlugin::default();
    Box::into_raw(Box::new(plugin))
}

// Core loads them with `libloading`:
unsafe {
    let lib = Library::new(path)?;
    let create: Symbol<unsafe extern "C" fn() -> *mut dyn JanusPlugin> =
        lib.get(b"janus_plugin_create")?;
    let plugin = Box::from_raw(create());
    // ...
}
```

---

## 5. Phased Implementation Plan

### Phase 0: Scaffolding (Week 1–2)

**Goal:** Cargo workspace compiles, CI runs, skeleton traits exist.

| Task | Details | Tests |
|------|---------|-------|
| Initialize workspace | `Cargo.toml` with all crate stubs | `cargo check` passes |
| Define `janus-plugin-api` | Plugin trait, callback trait, core types (`SessionId`, `RtpPacket`, `Jsep`, `PluginResult`) | Unit tests for types (serialize/deserialize, Display) |
| Define `janus-transport-api` | Transport trait, callback trait | Unit tests for types |
| Define `janus-event-api` | Event handler trait, `EventMask`, `JanusEvent` | Unit tests for event types |
| Config module | TOML parsing with serde, `JanusConfig` struct | Test: parse sample config, round-trip |
| CI pipeline | GitHub Actions: `cargo check`, `cargo test`, `cargo clippy`, `cargo fmt` | Matrix: stable + nightly |

**Tests for Phase 0:**
- `config::tests::parse_general_config`
- `config::tests::parse_plugin_config`
- `config::tests::invalid_config_errors`
- `types::tests::session_id_display`
- `types::tests::jsep_serialize_roundtrip`

---

### Phase 1: Core Server + EchoTest (Weeks 3–6)

**Goal:** A working server that can do WebRTC echo tests via WebSocket.

| Task | Details | Tests |
|------|---------|-------|
| Session manager | `DashMap<SessionId, Session>`, timeout watchdog | Unit: create/destroy/timeout |
| str0m integration | `WebRtcEngine` wrapping str0m for ICE+DTLS+SRTP | Unit: offer/answer cycle with mock |
| SDP module | Generate/parse SDP using str0m's SDP support | Unit: parse real-world SDPs from Janus test suite |
| RTP relay | Route incoming RTP to plugin, relay outgoing | Unit: mock plugin receives packets |
| Plugin loader | `libloading`-based `.so` scanner | Unit: load mock plugin .so |
| WebSocket transport | `tokio-tungstenite` implementing `JanusTransport` | Integration: WS connect + create session |
| EchoTest plugin | Port `janus_echotest.c` (1,463 lines) | Integration: full echo test round-trip |
| Admin API (basic) | List sessions, list plugins | Integration: HTTP request returns JSON |

**Tests for Phase 1:**
- `session::tests::create_session_returns_id`
- `session::tests::session_timeout_fires`
- `session::tests::concurrent_sessions` (tokio multi-threaded)
- `webrtc::tests::offer_answer_ice_lite`
- `webrtc::tests::dtls_handshake_completes`
- `sdp::tests::parse_chrome_offer`
- `sdp::tests::parse_firefox_offer`
- `sdp::tests::generate_answer_for_audio_video`
- `relay::tests::rtp_routed_to_plugin`
- `relay::tests::rtcp_routed_to_plugin`
- `loader::tests::load_valid_plugin`
- `loader::tests::reject_incompatible_plugin`
- `transport_ws::tests::connect_and_create_session`
- `echotest::tests::handle_message_enable_audio`
- `echotest::tests::handle_message_set_bitrate`
- **E2E:** `tests/echotest_e2e.rs` — full WebSocket → session → offer/answer → RTP echo

---

### Phase 2: VideoRoom + AudioBridge (Weeks 7–12)

**Goal:** Multi-party conferencing works (the two most important plugins).

| Task | Details | Tests |
|------|---------|-------|
| VideoRoom plugin | SFU: publish/subscribe, simulcast, room management (~14K lines to port) | Unit + integration per feature below |
| — Room management | Create/destroy/list rooms, participant tracking | Unit: CRUD ops on rooms |
| — Publisher flow | Offer → answer, RTP forwarding to subscribers | Integration: 2 publishers, 1 subscriber |
| — Subscriber flow | Subscribe to feeds, switch feeds | Integration: subscriber receives RTP |
| — Simulcast | Layer selection, temporal scalability | Unit: layer switching logic |
| — Recording | MJR-format recording | Unit: record and verify file |
| — RTP forwarders | Forward to external RTP endpoints | Integration: forward to UDP sink |
| AudioBridge plugin | MCU: Opus mixing, room management (~11K lines) | |
| — Room management | Create/destroy rooms | Unit: CRUD |
| — Opus mixing | Decode, mix N streams, re-encode | Unit: mix 2 silent + 1 tone = tone |
| — Spatial audio | Positional audio support | Unit: panning coefficients |
| HTTP transport | axum-based, Janus API + Admin API endpoints | Integration: long-poll + REST |

**Tests for Phase 2:**
- `videoroom::tests::create_room`
- `videoroom::tests::join_as_publisher`
- `videoroom::tests::join_as_subscriber`
- `videoroom::tests::publisher_rtp_reaches_subscriber`
- `videoroom::tests::simulcast_layer_switch`
- `videoroom::tests::room_destruction_cleans_up`
- `videoroom::tests::rtp_forwarder_sends_udp`
- `audiobridge::tests::create_room`
- `audiobridge::tests::mix_two_streams`
- `audiobridge::tests::spatial_audio_panning`
- `audiobridge::tests::opus_decode_encode_roundtrip`
- `transport_http::tests::long_poll_session`
- `transport_http::tests::admin_api_list_sessions`
- **E2E:** `tests/videoroom_e2e.rs` — 3-party video room
- **E2E:** `tests/audiobridge_e2e.rs` — 2-party audio mix

---

### Phase 3: Remaining Plugins (Weeks 13–18)

| Task | ~Lines to Port | Tests |
|------|---------------|-------|
| Streaming plugin | 10,818 | Unit: mount management, RTP relay; E2E: GStreamer → subscriber |
| SIP plugin | 8,464 | Unit: SIP message handling, SRTP offer; Integration: register + call flow (mock SIP peer) |
| NoSIP plugin | 3,299 | Unit: RTP bridge setup; Integration: generate offer, relay media |
| VideoCall plugin | 1,853 | Unit: registration, call/hangup; E2E: 1-to-1 call |
| RecordPlay plugin | 3,289 | Unit: MJR write/read; E2E: record then playback |
| TextRoom plugin | 3,245 | Unit: room CRUD, message routing; E2E: DataChannel text exchange |
| Lua plugin bridge | 2,587 | Unit: script loading, callback invocation (mlua) |

**Tests for Phase 3:**
- Per-plugin unit tests (room/session/message handling)
- Per-plugin integration tests (full signaling flows with mock peers)
- SIP-specific: mock SIP registrar + call agent
- Streaming: mock RTP source feeding into mount

---

### Phase 4: Transports + Event Handlers (Weeks 19–22)

| Task | Crate | Tests |
|------|-------|-------|
| MQTT transport | `rumqttc` | Unit: pub/sub message mapping; Integration: mosquitto in Docker |
| RabbitMQ transport | `lapin` | Unit: queue/exchange setup; Integration: RabbitMQ in Docker |
| Unix socket transport | `tokio::net::UnixStream` | Unit: connect, send, receive |
| Nanomsg transport | `nng` | Unit: pub/sub pattern |
| MQTT event handler | `rumqttc` | Integration: events appear on MQTT topic |
| RabbitMQ event handler | `lapin` | Integration: events in RabbitMQ queue |
| WebSocket event handler | `tokio-tungstenite` | Integration: events on WS connection |
| GELF event handler | UDP + `serde_json` | Unit: GELF message format; Integration: mock Graylog |
| Nanomsg event handler | `nng` | Unit: event serialization |
| Sample event handler | File/stdout | Unit: event formatting |

---

### Phase 5: Hardening + Parity (Weeks 23–28)

| Task | Details | Tests |
|------|---------|-------|
| Full admin API | All 50+ admin commands from C version | Integration: every command |
| Token auth | Stored token + per-plugin permissions | Unit: token CRUD, permission checks |
| TURN REST API | Credential generation for TURN servers | Unit: credential generation with HMAC |
| Oackback support | Support for RFC 8888 and abs-send-time | Unit: feedback packet parsing |
| Obackward compat | Optional `.jcfg` config parser for migration | Unit: parse all sample jcfg files |
| Graceful shutdown | Drain sessions, close transports, flush events | Integration: shutdown mid-session |
| Daemonize | Fork + PID file + signal handling | Integration: start as daemon, send SIGTERM |
| Log rotation | SIGHUP handler for log rotation | Unit: signal triggers rotate |
| Systemd integration | `sd_notify`, watchdog, socket activation | Integration: systemd unit file |

---

## 6. Testing Strategy

### 6.1 Test Pyramid

```
         ╱╲
        ╱  ╲         E2E tests (real browser via headless Chrome + WebDriver)
       ╱ E2E╲        ~20 tests, slow (~minutes)
      ╱──────╲
     ╱        ╲       Integration tests (in-process server, mock WebRTC peers)
    ╱ Integr.  ╲      ~100 tests, medium (~seconds)
   ╱────────────╲
  ╱              ╲     Unit tests (pure logic, no I/O)
 ╱    Unit        ╲    ~500+ tests, fast (~ms)
╱──────────────────╲
```

### 6.2 Unit Tests

Every module gets `#[cfg(test)] mod tests { ... }` with:

- **Happy path**: normal operation
- **Error cases**: invalid input, missing config, malformed SDP
- **Edge cases**: empty rooms, max participants, zero-length packets
- **Property tests** (proptest): fuzz SDP parsing, RTP header parsing, JSON message handling

```rust
// Example: property test for SDP roundtrip
proptest! {
    #[test]
    fn sdp_roundtrip(sdp in arb_sdp()) {
        let serialized = sdp.to_string();
        let parsed = Sdp::parse(&serialized).unwrap();
        assert_eq!(sdp, parsed);
    }
}
```

### 6.3 Integration Tests

Located in `tests/` directory. Each test:

1. Starts a `JanusServer` in-process with test config
2. Connects via WebSocket transport
3. Performs signaling (create session, attach plugin, send messages)
4. Uses mock WebRTC peer (str0m in client mode) for media
5. Asserts on plugin responses and media flow

```rust
// Example: echotest integration test
#[tokio::test]
async fn echotest_full_flow() {
    let server = TestServer::start(test_config()).await;
    let mut client = WsClient::connect(server.ws_url()).await;

    // Create session
    let session_id = client.create_session().await;

    // Attach echotest plugin
    let handle_id = client.attach(session_id, "janus.plugin.echotest").await;

    // Send offer
    let offer = generate_test_offer();
    let answer = client.send_message(session_id, handle_id, json!({
        "audio": true, "video": true
    }), Some(offer)).await;

    assert!(answer.jsep.is_some());
    assert_eq!(answer.jsep.unwrap().type_, "answer");

    // Verify media flows (with mock RTP)
    let mut peer = MockPeer::new(answer.jsep.unwrap());
    peer.send_rtp_audio(&[0u8; 160]).await;
    let echoed = peer.recv_rtp_audio().await;
    assert_eq!(echoed.payload_type(), 111); // Opus

    client.destroy_session(session_id).await;
    server.shutdown().await;
}
```

### 6.4 End-to-End Tests

Use headless Chrome via `chromiumoxide` or `fantoccini`:

```rust
#[tokio::test]
async fn e2e_videoroom_two_participants() {
    let server = TestServer::start(production_config()).await;

    let browser1 = Browser::launch_headless().await;
    let browser2 = Browser::launch_headless().await;

    browser1.navigate(&format!("{}/videoroomtest.html", server.http_url())).await;
    browser2.navigate(&format!("{}/videoroomtest.html", server.http_url())).await;

    // Wait for both to join room 1234
    browser1.click("#start").await;
    browser2.click("#start").await;

    // Assert: each sees the other's video
    browser1.wait_for("#remote-video-1").await;
    browser2.wait_for("#remote-video-1").await;

    browser1.close().await;
    browser2.close().await;
    server.shutdown().await;
}
```

### 6.5 Benchmark Tests

Using `criterion`:

```rust
fn bench_rtp_relay(c: &mut Criterion) {
    c.bench_function("relay_1000_rtp_packets", |b| {
        b.iter(|| {
            let mut relay = RtpRelay::new();
            for _ in 0..1000 {
                relay.forward(black_box(&test_rtp_packet()));
            }
        })
    });
}
```

Key benchmarks:
- RTP packet relay throughput
- SDP parse/generate latency
- Session creation/destruction
- VideoRoom: relay fan-out to N subscribers
- AudioBridge: mix N Opus streams

### 6.6 Compatibility Tests

Verify wire-level compatibility with the C version:

```rust
/// Captured from real C Janus ↔ Chrome session
const CHROME_OFFER: &str = include_str!("fixtures/chrome_offer.sdp");
const JANUS_ANSWER: &str = include_str!("fixtures/janus_c_answer.sdp");

#[test]
fn answer_compatible_with_c_janus() {
    let offer = Sdp::parse(CHROME_OFFER).unwrap();
    let answer = generate_answer(&offer, &default_config());
    // Verify critical fields match C version behavior
    assert!(answer.has_ice_lite());
    assert_eq!(answer.media_count(), offer.media_count());
    for m in answer.media() {
        assert!(m.has_rtcp_mux());
    }
}
```

### 6.7 Test Infrastructure

```rust
// tests/common/mod.rs

/// Spin up a full Janus server for integration tests.
pub struct TestServer { /* ... */ }

impl TestServer {
    pub async fn start(config: JanusConfig) -> Self { /* ... */ }
    pub fn ws_url(&self) -> String { /* ... */ }
    pub fn http_url(&self) -> String { /* ... */ }
    pub async fn shutdown(self) { /* ... */ }
}

/// WebSocket client for test signaling.
pub struct WsClient { /* ... */ }

impl WsClient {
    pub async fn connect(url: &str) -> Self { /* ... */ }
    pub async fn create_session(&mut self) -> SessionId { /* ... */ }
    pub async fn attach(&mut self, session: SessionId, plugin: &str) -> HandleId { /* ... */ }
    pub async fn send_message(/* ... */) -> PluginResponse { /* ... */ }
    pub async fn destroy_session(&mut self, session: SessionId) { /* ... */ }
}

/// Mock WebRTC peer for media-level testing.
pub struct MockPeer { /* ... */ }
```

---

## 7. Dependency Map

```toml
# Cargo workspace dependencies
[workspace.dependencies]
# Async
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"

# WebRTC
str0m = "0.11"                    # ICE + DTLS + SRTP + SCTP + SDP

# Web / API
axum = "0.7"                      # HTTP transport
tokio-tungstenite = "0.24"        # WebSocket transport
tower = "0.4"                     # middleware for axum
hyper = "1"                       # HTTP primitives

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Messaging
rumqttc = "0.24"                  # MQTT
lapin = "2"                       # RabbitMQ (AMQP)
nng = "1"                         # nanomsg-next-gen

# SIP
rsip = "0.5"                      # SIP message parsing
# sofia-sip-sys = "0.1"           # FFI to sofia-sip (Phase 3)

# Audio
audiopus = "0.3"                  # Opus codec FFI

# Scripting
mlua = { version = "0.9", features = ["lua54"] }  # Lua bridge

# Plugin loading
libloading = "0.8"                # dlopen/dlsym

# Data structures
dashmap = "6"                     # concurrent HashMap
uuid = { version = "1", features = ["v4"] }

# Crypto / TLS
rustls = "0.23"                   # TLS for transports
ring = "0.17"                     # HMAC for TURN REST API

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Testing
proptest = "1"
criterion = "0.5"
tokio-test = "0.4"
chromiumoxide = "0.7"             # headless browser for E2E
```

---

## 8. Migration & Compatibility

### 8.1 Config Migration

Provide a `janus-migrate` CLI tool:

```bash
# Convert old jcfg configs to new TOML format
janus-migrate config --input /etc/janus/ --output /etc/janus-rs/
```

### 8.2 Plugin API Compatibility

The Rust plugin API is intentionally similar to the C API. A migration guide
maps every C callback to its Rust trait method:

| C callback | Rust trait method |
|------------|-------------------|
| `init(callbacks, config_path)` | `async fn init(&mut self, callbacks, config_path)` |
| `handle_message(handle, transaction, message, jsep)` | `async fn handle_message(session_id, transaction, body, jsep)` |
| `incoming_rtp(handle, packet)` | `fn incoming_rtp(session_id, packet)` |
| ... | ... |

### 8.3 Wire Compatibility

The JSON signaling protocol is identical — existing client-side JavaScript
(janus.js) works without modification against the Rust server.

---

## 9. Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| str0m missing a feature (e.g., specific codec negotiation) | Medium | str0m is actively maintained; fallback: contribute upstream or use webrtc-rs |
| SIP plugin complexity (sofia-sip dependency) | High | Phase 1: FFI bridge to sofia-sip; Phase 2: gradual pure-Rust replacement |
| Dynamic plugin loading with Rust traits across `.so` boundaries | Medium | Careful ABI: use `#[repr(C)]` wrappers or `abi_stable` crate |
| Performance regression vs C for audio mixing | Low | Benchmark early; Opus encoding is FFI to C anyway |
| GLib → tokio migration (event loops, thread pools) | Medium | tokio's multi-threaded runtime is equivalent; careful with blocking ops |

---

## 10. Timeline Summary

| Phase | Weeks | Milestone |
|-------|-------|-----------|
| 0 — Scaffolding | 1–2 | Workspace compiles, traits defined, CI green |
| 1 — Core + EchoTest | 3–6 | Working WebRTC echo test via WebSocket |
| 2 — VideoRoom + AudioBridge | 7–12 | Multi-party video/audio conferencing |
| 3 — Remaining Plugins | 13–18 | All 11 plugins ported |
| 4 — Transports + Events | 19–22 | All 6 transports, all 6 event handlers |
| 5 — Hardening | 23–28 | Admin API, auth, graceful shutdown, benchmarks |

**Total: ~28 weeks for full feature parity with tests.**

---

## 11. Definition of Done

A phase is complete when:

1. All `cargo test` pass (unit + integration)
2. `cargo clippy -- -D warnings` is clean
3. `cargo fmt --check` passes
4. No `unsafe` outside of FFI boundaries
5. All public APIs have doc comments
6. Benchmark baselines established (Phase 1+)
7. E2E tests pass against headless Chrome (Phase 1+)

---

## 12. Progress Log

### Phase 0 + 1 — DONE

**Status:** 94 tests passing, all crates compile.

**Crates implemented:**

| Crate | Lines | Tests | Status |
|-------|-------|-------|--------|
| `janus-plugin-api` | ~400 | 19 (incl. 5 proptest) | Done |
| `janus-transport-api` | ~120 | 4 | Done |
| `janus-event-api` | ~170 | 7 | Done |
| `janus-core` | ~900 | 45 | Done |
| `janus-plugin-echotest` | ~270 | 13 | Done |
| `janus-transport-http` | ~170 | 4 | Done |
| `janus-transport-websocket` | ~230 | 2 | Done |
| **Total** | **~2,260** | **94** | |

**What works:**
- Cargo workspace with 7 crates
- Plugin API trait with full lifecycle (init, create_session, handle_message, media callbacks)
- Transport API trait with callbacks into core
- Event handler API trait with event mask system
- TOML config parsing with all major Janus config sections
- Session manager with timeout watchdog, concurrent DashMap storage
- Plugin loader (libloading-based dynamic .so loading)
- RTP/RTCP relay engine with per-handle statistics
- Core server processing: ping, info, create, destroy, attach, detach, keepalive
- API secret authentication
- EchoTest plugin: audio/video mute, RTP echo, JSEP offer/answer, session lifecycle
- HTTP transport (axum): REST API with session/handle path routing
- WebSocket transport (tokio-tungstenite): full bidirectional JSON messaging

**Phase 1 completed:**
- Main binary entry point (`janus-rs`)
- Integration tests: full HTTP + WS echotest round-trip
- CI workflow (GitHub Actions)

**Phase 2 completed — WebRTC Core + Message Routing:**
- str0m WebRTC integration (`webrtc.rs`, `sdp.rs`)
- PeerConnection actor pattern with tokio tasks
- Full plugin message dispatch (server.rs rewrite)
- Event push routing to transports (WS event sender + HTTP long-poll)
- EchoTest plugin updated to OkWait JSEP pattern

**Phase 3 completed — VideoRoom Plugin:**
- Full SFU VideoRoom plugin with publisher/subscriber model
- Room CRUD, join/leave, publish/subscribe, RTP fan-out
- Wire-compatible with janus.js VideoRoom API
- Default room 1234 auto-created on init

**Phase 4 completed — Streaming Plugin:**
- Mountpoint CRUD, viewer lifecycle, RTP relay
- Watch/start/pause/stop commands
- Default mountpoint 1 (audio port 5004, video port 5006)

**Phase 5 completed — Plugin Stubs:**
- AudioBridge, SIP, NoSIP, VideoCall, Record&Play, TextRoom stubs
- All implement JanusPlugin trait with NotImplemented handle_message

**Phase 6 completed — Docker Deployment:**
- Multi-stage Dockerfile (rust:1.83-bookworm builder, debian:bookworm-slim runtime)
- docker-compose.yml with port mappings
- Static file serving via tower-http ServeDir
- TOML config at config/janus.toml

**Phase 7 completed — Demo Website:**
- Bouncing ball + 440 Hz sinewave VideoRoom demo (html/rust-demos/)
- Streaming viewer demo
- WHIP/WHEP publisher + subscriber demo (no janus.js)
- Standalone WHEP player demo (no janus.js)
- Uses janus.js from existing demos for VideoRoom/Streaming demos

**Phase 8 completed — E2E Tests:**
- Playwright test suite (server health, echotest, videoroom, streaming, WHIP/WHEP)
- 27 E2E tests across `tests/e2e/*.spec.ts`
- GitHub Actions workflow (.github/workflows/e2e-ci.yml)

**Phase 9 completed — WHIP/WHEP Plugin:**
- RFC 9725 WHIP ingest (publish via HTTP POST of SDP offer)
- WHEP egress (subscribe to published streams via HTTP POST)
- Resource management: trickle ICE, ICE restart, DELETE teardown
- Link headers with ICE server info from NatConfig (STUN/TURN)
- SDP fragment parsing for trickle ICE candidates
- Fan-out manager for publisher → subscriber RTP relay
- 53 unit tests in `janus-whip-whep` crate

**Phase 10 completed — WHIP-Out Relay:**
- Outgoing WHIP client: server acts as WHIP client to push media to remote endpoints
- `POST /whip-out/:publisher_id` — add a relay target (negotiates WebRTC with remote)
- `GET /whip-out/:publisher_id` — list active relay targets
- Cascade cleanup: deleting a publisher also tears down all WHIP-out relays (remote DELETE)
- Fixed `SendRtp` stub in PeerConnectionActor — now writes actual RTP via str0m `writer.write()`
- Added offerer flow: `CreateOffer` + `SetRemoteAnswer` PcCommand variants
- Added `reqwest` dependency for outgoing HTTP signaling
- 81 tests in `janus-core`, 53 tests in `janus-whip-whep`

**Phase 11 completed — Meshcast Interop Tests:**
- 4 Playwright tests in `tests/e2e/meshcast-whip-whep.spec.ts`
- Validates WHIP/WHEP against Meshcast external server (real WebRTC: ICE, DTLS, RTP)
- WHIP publish → ICE connected, WHEP subscribe → video frames, round-trip, DELETE cleanup
- Separate `meshcast` Playwright project (not in default run, requires internet)
- Graceful skip when Meshcast is unreachable (OPTIONS probe in beforeAll)

**Current workspace: 17 crates, 286+ Rust unit/integration tests, 31 Playwright E2E tests (27 local + 4 Meshcast interop), all passing.**
