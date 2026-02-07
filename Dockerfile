# Stage 1: Build the Rust binary
FROM rust:1.83-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Build release binary
RUN cargo build --release -p janus-server

# Stage 2: Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/janus

# Copy binary
COPY --from=builder /build/target/release/janus-rs /usr/local/bin/janus-rs

# Copy static web files
COPY html/ /opt/janus/html/

# Copy config
COPY config/ /etc/janus/

# Set environment
ENV JANUS_CONFIG=/etc/janus/janus.toml
ENV JANUS_STATIC_DIR=/opt/janus/html
ENV RUST_LOG=info

# HTTP API
EXPOSE 8088
# WebSocket API
EXPOSE 8188
# RTP ports for streaming plugin
EXPOSE 5004/udp
EXPOSE 5006/udp
# WebRTC UDP range
EXPOSE 20000-20100/udp

CMD ["janus-rs"]
