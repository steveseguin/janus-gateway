//! Janus Gateway core server.
//!
//! This crate contains the main server loop, session management, plugin
//! loading, media relay, and the glue between transports and plugins.

pub mod config;
pub mod error;
pub mod plugin_loader;
pub mod relay;
pub mod sdp;
pub mod server;
pub mod session;
pub mod webrtc;

pub use error::{Error, Result};
