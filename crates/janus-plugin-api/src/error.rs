//! Error types for the Janus plugin API.

use thiserror::Error;

/// Plugin API error type.
#[derive(Debug, Error)]
pub enum Error {
    #[error("not implemented")]
    NotImplemented,

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("missing mandatory field: {0}")]
    MissingField(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience Result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let err = Error::NotImplemented;
        assert_eq!(err.to_string(), "not implemented");

        let err = Error::InvalidRequest("bad body".into());
        assert_eq!(err.to_string(), "invalid request: bad body");

        let err = Error::SessionNotFound("123".into());
        assert_eq!(err.to_string(), "session not found: 123");

        let err = Error::MissingField("room".into());
        assert_eq!(err.to_string(), "missing mandatory field: room");
    }

    #[test]
    fn error_from_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("{bad").unwrap_err();
        let err: Error = json_err.into();
        assert!(matches!(err, Error::Json(_)));
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Error>();
    }
}
