//! Core error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("session not found: {0}")]
    SessionNotFound(u64),

    #[error("handle not found: {0}")]
    HandleNotFound(u64),

    #[error("plugin not found: {0}")]
    PluginNotFound(String),

    #[error("plugin load error: {0}")]
    PluginLoad(String),

    #[error("no transports available")]
    NoTransports,

    #[error("configuration error: {0}")]
    Config(String),

    #[error("server already running")]
    AlreadyRunning,

    #[error("server not running")]
    NotRunning,

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error(transparent)]
    Plugin(#[from] janus_plugin_api::Error),

    #[error(transparent)]
    Transport(#[from] janus_transport_api::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        assert_eq!(
            Error::SessionNotFound(123).to_string(),
            "session not found: 123"
        );
        assert_eq!(
            Error::PluginNotFound("janus.plugin.foo".into()).to_string(),
            "plugin not found: janus.plugin.foo"
        );
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Error>();
    }
}
