//! Janus Gateway — Rust implementation.
//!
//! This binary starts the Janus server with configured transports and plugins.

use janus_core::config::JanusConfig;
use janus_core::server::JanusServer;
use janus_plugin_api::JanusPlugin;
use janus_plugin_echotest::EchoTestPlugin;
use janus_plugin_streaming::StreamingPlugin;
use janus_plugin_videoroom::VideoRoomPlugin;
use janus_transport_http::{start_http_transport, HttpTransportConfig};
use janus_transport_websocket::{WsTransport, WsTransportConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Janus Gateway (Rust) starting...");

    // Load config — try env var, then default path, then fallback to defaults
    let config = load_config();

    info!(
        server_name = %config.general.server_name,
        session_timeout = config.general.session_timeout,
        "configuration loaded"
    );

    // Create the core server
    let server = Arc::new(JanusServer::new(config));
    let plugin_config_dir = server.config().general.configs_folder.clone();

    // Register built-in plugins
    let mut echotest = EchoTestPlugin::default();
    let plugin_callbacks = server.plugin_callbacks();
    if let Err(e) = echotest.init(plugin_callbacks, &plugin_config_dir).await {
        error!(error = %e, "failed to init EchoTest plugin");
    } else {
        server.register_plugin(Arc::new(echotest));
    }

    let mut videoroom = VideoRoomPlugin::default();
    let plugin_callbacks = server.plugin_callbacks();
    if let Err(e) = videoroom.init(plugin_callbacks, &plugin_config_dir).await {
        error!(error = %e, "failed to init VideoRoom plugin");
    } else {
        server.register_plugin(Arc::new(videoroom));
    }

    let mut streaming = StreamingPlugin::default();
    let plugin_callbacks = server.plugin_callbacks();
    if let Err(e) = streaming.init(plugin_callbacks, &plugin_config_dir).await {
        error!(error = %e, "failed to init Streaming plugin");
    } else {
        server.register_plugin(Arc::new(streaming));
    }

    // Start HTTP transport with static file serving
    let mut http_config = HttpTransportConfig::default();
    // Serve static files from JANUS_STATIC_DIR env or default "html" directory
    let static_dir = std::env::var("JANUS_STATIC_DIR").unwrap_or_else(|_| "html".into());
    if std::path::Path::new(&static_dir).is_dir() {
        info!(directory = %static_dir, "enabling static file serving");
        http_config.static_dir = Some(static_dir);
    }
    if let Err(e) =
        start_http_transport(&http_config, Arc::clone(&server), &server.config().nat).await
    {
        error!(error = %e, "failed to start HTTP transport");
    }

    // Start WebSocket transport
    let ws_config = WsTransportConfig::default();
    let ws_transport = WsTransport::new(ws_config, Arc::clone(&server));
    if let Err(e) = ws_transport.start().await {
        error!(error = %e, "failed to start WebSocket transport");
    }

    info!("Janus Gateway (Rust) ready");
    info!("  HTTP API:      http://0.0.0.0:8088/janus");
    info!("  WebSocket API: ws://0.0.0.0:8188");
    info!("Press Ctrl+C to stop");

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received Ctrl+C, shutting down...");
        }
        _ = server.wait_for_shutdown() => {
            info!("shutdown signal received");
        }
    }

    info!("Janus Gateway (Rust) stopped");
}

fn load_config() -> JanusConfig {
    // Check JANUS_CONFIG env var
    if let Ok(path) = std::env::var("JANUS_CONFIG") {
        match JanusConfig::from_file(&PathBuf::from(&path)) {
            Ok(config) => {
                info!(path = %path, "loaded config from JANUS_CONFIG");
                return config;
            }
            Err(e) => {
                error!(path = %path, error = %e, "failed to load config from JANUS_CONFIG");
            }
        }
    }

    // Try default path
    let default_path = PathBuf::from("/etc/janus/janus.toml");
    if default_path.exists() {
        match JanusConfig::from_file(&default_path) {
            Ok(config) => {
                info!(path = %default_path.display(), "loaded config from default path");
                return config;
            }
            Err(e) => {
                error!(
                    path = %default_path.display(),
                    error = %e,
                    "failed to load default config"
                );
            }
        }
    }

    info!("using default configuration");
    JanusConfig::default()
}
