Janus WebRTC Server — Rust Rewrite
===================================
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-brightgreen.svg)](COPYING)

Janus is an open source, general purpose, WebRTC server originally designed and developed by [Meetecho](https://www.meetecho.com). This fork is a **ground-up rewrite in Rust**, replacing the C implementation with a safe, async, high-performance Rust workspace while preserving wire-level compatibility with existing Janus clients (janus.js).

> **Note:** The original C source is preserved in `c-legacy/` for reference. All active development happens in the Rust workspace.

## Architecture

```
janus-gateway/
├── Cargo.toml                     # Workspace root (17 crates)
├── crates/
│   ├── janus-core/                # Core: sessions, WebRTC (str0m), config, SDP
│   ├── janus-plugin-api/          # Plugin trait definitions + types
│   ├── janus-transport-api/       # Transport trait definitions
│   ├── janus-event-api/           # Event handler trait definitions
│   ├── janus-whip-whep/           # WHIP ingest + WHEP egress + WHIP-out relay
│   ├── janus-server/              # Binary entry point (janus-rs)
│   ├── plugins/
│   │   ├── janus-plugin-echotest/     # Full: audio/video echo
│   │   ├── janus-plugin-videoroom/    # Full: SFU multi-party conferencing
│   │   ├── janus-plugin-streaming/    # Full: RTP mountpoint streaming
│   │   ├── janus-plugin-audiobridge/  # Stub
│   │   ├── janus-plugin-sip/         # Stub
│   │   ├── janus-plugin-nosip/       # Stub
│   │   ├── janus-plugin-videocall/   # Stub
│   │   ├── janus-plugin-recordplay/  # Stub
│   │   └── janus-plugin-textroom/    # Stub
│   └── transports/
│       ├── janus-transport-http/      # axum REST + static file serving
│       └── janus-transport-websocket/ # tokio-tungstenite WebSocket
├── html/rust-demos/               # Demo web pages
├── tests/e2e/                     # Playwright E2E tests
├── config/                        # TOML configuration
├── c-legacy/                      # Original C source (archived)
└── RUST_REWRITE_PLAN.md           # Detailed rewrite plan + progress
```

## Key Technologies

| Component | Technology |
|-----------|-----------|
| WebRTC stack | [str0m](https://github.com/algesten/str0m) (sans-I/O: ICE + DTLS + SRTP + SCTP) |
| Async runtime | [tokio](https://tokio.rs/) |
| HTTP transport | [axum](https://github.com/tokio-rs/axum) |
| WebSocket transport | [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite) |
| Config format | TOML via [serde](https://serde.rs/) |
| Concurrency | [DashMap](https://github.com/xacrimon/dashmap) + tokio tasks |

## Features

### Working Today
- **Core server**: session management, plugin dispatch, API secret auth, TOML config
- **WebRTC**: full PeerConnection lifecycle via str0m (offer/answer, ICE, DTLS, SRTP)
- **EchoTest plugin**: audio/video echo with mute control
- **VideoRoom plugin**: full SFU with publisher/subscriber model, room CRUD, RTP fan-out
- **Streaming plugin**: RTP mountpoint management, viewer lifecycle
- **WHIP/WHEP**: RFC 9725 WHIP ingest + WHEP egress (no janus.js required)
- **WHIP-out relay**: forward incoming WHIP streams to remote WHIP endpoints
- **HTTP transport**: REST API + static file serving via tower-http
- **WebSocket transport**: bidirectional JSON messaging
- **Demo pages**: bouncing ball VideoRoom, streaming viewer, WHIP/WHEP publisher+subscriber, standalone WHEP player
- **E2E tests**: Playwright test suite (27 local tests + 4 Meshcast interop tests)
- **286+ Rust unit/integration tests** across all crates

### Plugin Stubs (API scaffolded, returns NotImplemented)
AudioBridge, SIP, NoSIP, VideoCall, Record&Play, TextRoom

## Quick Start

### Prerequisites
- [Rust](https://rustup.rs/) (stable, 2021 edition)
- For E2E tests: Node.js 18+, Chromium

### Build and Run

```bash
# Build the server
cargo build --release

# Run with default config
cargo run --release -p janus-server

# The server listens on:
#   HTTP API:  http://localhost:8088
#   WebSocket: ws://localhost:8188
#   WHIP:      POST http://localhost:8088/whip
#   WHEP:      POST http://localhost:8088/whep/:publisher_id
```

### Run Tests

```bash
# All Rust tests
cargo test --workspace

# Just the WHIP/WHEP plugin tests
cargo test -p janus-whip-whep

# E2E tests (requires running server + Chromium)
cd tests/e2e
npm install
npx playwright install chromium
npx playwright test

# Meshcast interop tests (requires internet, no local server needed)
npx playwright test --project=meshcast
```

### Docker

```bash
docker-compose up --build
```

## WHIP/WHEP API

The server implements [RFC 9725 (WHIP)](https://www.rfc-editor.org/rfc/rfc9725) and WHEP for browser-friendly WebRTC publishing and subscribing without janus.js.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/whip` | POST | Publish media (SDP offer → SDP answer) |
| `/whep/:publisher_id` | POST | Subscribe to a publisher (SDP offer → SDP answer) |
| `/resource/:id` | PATCH | Trickle ICE candidates / ICE restart |
| `/resource/:id` | DELETE | Tear down a session |
| `/whip-out/:publisher_id` | POST | Add outgoing WHIP relay to a remote endpoint |
| `/whip-out/:publisher_id` | GET | List active WHIP-out relays for a publisher |

### WHIP-Out Relay

Forward a local WHIP stream to one or more remote WHIP endpoints:

```bash
# Add a relay target
curl -X POST http://localhost:8088/whip-out/<publisher-id> \
  -H "Content-Type: application/json" \
  -d '{"endpoint": "https://remote-server/whip", "bearer_token": "optional-token"}'

# List relays for a publisher
curl http://localhost:8088/whip-out/<publisher-id>

# Delete a relay (same as any resource)
curl -X DELETE http://localhost:8088/resource/<relay-id>
```

## Configuration

The server uses TOML configuration at `config/janus.toml`. See the file for all available options including NAT/STUN/TURN settings, ICE-lite mode, and plugin configuration.

## Demo Pages

Open `http://localhost:8088/` after starting the server to access:

- **Bouncing Ball** — VideoRoom demo with synthetic video + audio (uses janus.js)
- **Streaming Viewer** — Watch RTP streaming mountpoints (uses janus.js)
- **WHIP/WHEP** — Publish camera via WHIP, subscribe via WHEP (no janus.js)
- **WHEP Player** — Standalone WHEP subscriber (no janus.js)

## Documentation

- [RUST_REWRITE_PLAN.md](RUST_REWRITE_PLAN.md) — Full rewrite plan with architecture, trait definitions, and progress log
- [CHANGELOG.md](CHANGELOG.md) — Release history
- [Original Janus docs](https://janus.conf.meetecho.com/docs/) — C version API reference (wire protocol is compatible)

## Contributing

This is an active rewrite. The original C Janus contribution guidelines in `.github/CONTRIBUTING.md` still apply for code style and issue reporting.

## License

GPL-3.0 — see [COPYING](COPYING).

Originally developed by [@meetecho](https://github.com/meetecho). Rust rewrite by [@steveseguin](https://github.com/steveseguin).
