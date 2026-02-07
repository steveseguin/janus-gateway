//! Error types for the Janus transport API.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("transport not initialized")]
    NotInitialized,

    #[error("client not found: {0}")]
    ClientNotFound(String),

    #[error("send failed: {0}")]
    SendFailed(String),

    #[error("configuration error: {0}")]
    Config(String),

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
            Error::NotInitialized.to_string(),
            "transport not initialized"
        );
        assert_eq!(
            Error::ClientNotFound("abc".into()).to_string(),
            "client not found: abc"
        );
    }
}
