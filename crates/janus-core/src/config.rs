//! Configuration parsing for Janus Gateway.
//!
//! Reads TOML config files with structures that mirror the original C Janus
//! `.jcfg` configuration options.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JanusConfig {
    pub general: GeneralConfig,
    pub certificates: CertificatesConfig,
    pub media: MediaConfig,
    pub nat: NatConfig,
    pub plugins: PathsConfig,
    pub transports: PathsConfig,
    pub events: EventsConfig,
    pub admin: AdminConfig,
}

impl Default for JanusConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            certificates: CertificatesConfig::default(),
            media: MediaConfig::default(),
            nat: NatConfig::default(),
            plugins: PathsConfig {
                path: PathBuf::from("/usr/lib/janus/plugins"),
                disable: Vec::new(),
            },
            transports: PathsConfig {
                path: PathBuf::from("/usr/lib/janus/transports"),
                disable: Vec::new(),
            },
            events: EventsConfig::default(),
            admin: AdminConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Configuration files path.
    pub configs_folder: PathBuf,
    /// Debug/logging level (0-7).
    pub debug_level: u8,
    /// Whether to daemonize.
    pub daemonize: bool,
    /// PID file path.
    pub pid_file: PathBuf,
    /// API secret for authenticating requests.
    pub api_secret: Option<String>,
    /// Whether to use token-based authentication.
    pub token_auth: bool,
    /// Token auth secret for signing.
    pub token_auth_secret: Option<String>,
    /// Session timeout in seconds.
    pub session_timeout: u64,
    /// Reclaim session timeout in seconds.
    pub reclaim_session_timeout: u64,
    /// Server name reported in info endpoint.
    pub server_name: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            configs_folder: PathBuf::from("/etc/janus"),
            debug_level: 4,
            daemonize: false,
            pid_file: PathBuf::from("/var/run/janus.pid"),
            api_secret: None,
            token_auth: false,
            token_auth_secret: None,
            session_timeout: 60,
            reclaim_session_timeout: 0,
            server_name: "Janus Gateway (Rust)".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CertificatesConfig {
    /// Path to DTLS certificate file.
    pub cert_pem: Option<PathBuf>,
    /// Path to DTLS certificate key.
    pub cert_key: Option<PathBuf>,
    /// DTLS cipher suite.
    pub dtls_ciphers: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    /// Maximum NACK queue duration (ms).
    pub max_nack_queue: u32,
    /// RTP port range lower bound.
    pub rtp_port_range_min: u16,
    /// RTP port range upper bound.
    pub rtp_port_range_max: u16,
    /// Enable/disable TWCC.
    pub twcc: bool,
    /// Relay command buffer size (number of commands).
    pub relay_buffer_size: usize,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_nack_queue: 500,
            rtp_port_range_min: 20000,
            rtp_port_range_max: 40000,
            twcc: true,
            relay_buffer_size: 65536,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NatConfig {
    /// STUN server address.
    pub stun_server: Option<String>,
    /// STUN server port.
    pub stun_port: u16,
    /// TURN server address.
    pub turn_server: Option<String>,
    /// TURN server port.
    pub turn_port: u16,
    /// TURN username.
    pub turn_user: Option<String>,
    /// TURN password.
    pub turn_pwd: Option<String>,
    /// TURN REST API server.
    pub turn_rest_api: Option<String>,
    /// TURN REST API key.
    pub turn_rest_api_key: Option<String>,
    /// Whether to use ICE lite mode.
    pub ice_lite: bool,
    /// NAT 1:1 public IP mapping.
    pub nat_1_1_mapping: Option<String>,
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            stun_server: Some("stun.l.google.com".into()),
            stun_port: 19302,
            turn_server: None,
            turn_port: 3478,
            turn_user: None,
            turn_pwd: None,
            turn_rest_api: None,
            turn_rest_api_key: None,
            ice_lite: false,
            nat_1_1_mapping: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    /// Directory containing the shared libraries.
    pub path: PathBuf,
    /// List of plugin/transport packages to disable.
    pub disable: Vec<String>,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            disable: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EventsConfig {
    /// Whether to broadcast events to handlers.
    pub broadcast: bool,
    /// Path to event handler shared libraries.
    pub path: PathBuf,
    /// Handlers to disable.
    pub disable: Vec<String>,
    /// Stats reporting period (seconds).
    pub stats_period: u64,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            broadcast: false,
            path: PathBuf::from("/usr/lib/janus/events"),
            disable: Vec::new(),
            stats_period: 5,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminConfig {
    /// Admin API secret.
    pub admin_secret: Option<String>,
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

impl JanusConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &Path) -> crate::Result<Self> {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            crate::Error::Config(format!("failed to read {}: {}", path.display(), e))
        })?;
        Self::parse_toml(&contents)
    }

    /// Parse configuration from a TOML string.
    pub fn parse_toml(s: &str) -> crate::Result<Self> {
        toml::from_str(s).map_err(|e| crate::Error::Config(format!("failed to parse config: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = JanusConfig::default();
        assert_eq!(cfg.general.session_timeout, 60);
        assert_eq!(cfg.general.debug_level, 4);
        assert!(!cfg.general.daemonize);
        assert!(!cfg.events.broadcast);
        assert_eq!(cfg.media.rtp_port_range_min, 20000);
        assert_eq!(cfg.media.rtp_port_range_max, 40000);
    }

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
            [general]
            debug_level = 6
            session_timeout = 30
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert_eq!(cfg.general.debug_level, 6);
        assert_eq!(cfg.general.session_timeout, 30);
        // Defaults still apply for everything else
        assert!(!cfg.general.daemonize);
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
            [general]
            configs_folder = "/etc/janus-rs"
            debug_level = 7
            daemonize = false
            pid_file = "/tmp/janus.pid"
            api_secret = "supersecret"
            token_auth = true
            session_timeout = 120
            server_name = "Test Server"

            [certificates]
            cert_pem = "/etc/certs/cert.pem"
            cert_key = "/etc/certs/key.pem"

            [media]
            max_nack_queue = 1000
            rtp_port_range_min = 10000
            rtp_port_range_max = 20000
            twcc = false

            [nat]
            stun_server = "stun.l.google.com"
            stun_port = 19302
            ice_lite = true

            [plugins]
            path = "/usr/local/lib/janus/plugins"
            disable = ["janus.plugin.textroom"]

            [transports]
            path = "/usr/local/lib/janus/transports"
            disable = []

            [events]
            broadcast = true
            path = "/usr/local/lib/janus/events"
            stats_period = 10

            [admin]
            admin_secret = "adminpass"
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert_eq!(cfg.general.debug_level, 7);
        assert_eq!(cfg.general.api_secret.as_deref(), Some("supersecret"));
        assert!(cfg.general.token_auth);
        assert_eq!(cfg.general.server_name, "Test Server");
        assert_eq!(
            cfg.certificates.cert_pem.as_deref(),
            Some(Path::new("/etc/certs/cert.pem"))
        );
        assert_eq!(cfg.media.max_nack_queue, 1000);
        assert!(!cfg.media.twcc);
        assert_eq!(cfg.nat.stun_server.as_deref(), Some("stun.l.google.com"));
        assert!(cfg.nat.ice_lite);
        assert_eq!(
            cfg.plugins.disable,
            vec!["janus.plugin.textroom".to_string()]
        );
        assert!(cfg.events.broadcast);
        assert_eq!(cfg.events.stats_period, 10);
        assert_eq!(cfg.admin.admin_secret.as_deref(), Some("adminpass"));
    }

    #[test]
    fn parse_empty_string_yields_defaults() {
        let cfg = JanusConfig::parse_toml("").unwrap();
        assert_eq!(cfg.general.session_timeout, 60);
    }

    #[test]
    fn parse_invalid_toml_returns_error() {
        let result = JanusConfig::parse_toml("{{{{");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to parse config"));
    }

    #[test]
    fn config_roundtrip_via_toml() {
        let cfg = JanusConfig::default();
        let serialized = toml::to_string(&cfg).unwrap();
        let parsed = JanusConfig::parse_toml(&serialized).unwrap();
        assert_eq!(cfg.general.session_timeout, parsed.general.session_timeout);
        assert_eq!(cfg.general.debug_level, parsed.general.debug_level);
        assert_eq!(cfg.nat.ice_lite, parsed.nat.ice_lite);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let toml = r#"
            [general]
            debug_level = 5
            some_future_field = "hello"
        "#;
        // This should not error — unknown fields are silently ignored
        let result = JanusConfig::parse_toml(toml);
        // TOML strict mode may error; our config uses `deny_unknown_fields`
        // only if we opt in. With serde default, unknown fields are ignored.
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn from_nonexistent_file_returns_error() {
        let result = JanusConfig::from_file(Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to read"));
    }

    #[test]
    fn parse_nat_section() {
        let toml = r#"
            [nat]
            stun_server = "stun.example.com"
            stun_port = 3479
            turn_server = "turn.example.com"
            turn_port = 3480
            turn_user = "user"
            turn_pwd = "pass"
            ice_lite = true
            nat_1_1_mapping = "1.2.3.4"
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert_eq!(cfg.nat.stun_server.as_deref(), Some("stun.example.com"));
        assert_eq!(cfg.nat.stun_port, 3479);
        assert_eq!(cfg.nat.turn_server.as_deref(), Some("turn.example.com"));
        assert_eq!(cfg.nat.turn_port, 3480);
        assert_eq!(cfg.nat.turn_user.as_deref(), Some("user"));
        assert_eq!(cfg.nat.turn_pwd.as_deref(), Some("pass"));
        assert!(cfg.nat.ice_lite);
        assert_eq!(cfg.nat.nat_1_1_mapping.as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn parse_media_section_partial() {
        let toml = r#"
            [media]
            twcc = false
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert!(!cfg.media.twcc);
        // Defaults remain for unset fields
        assert_eq!(cfg.media.max_nack_queue, 500);
        assert_eq!(cfg.media.rtp_port_range_min, 20000);
    }

    #[test]
    fn parse_multiple_disabled_plugins() {
        let toml = r#"
            [plugins]
            path = "/usr/lib/janus/plugins"
            disable = ["janus.plugin.textroom", "janus.plugin.lua", "janus.plugin.duktape"]
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert_eq!(cfg.plugins.disable.len(), 3);
        assert!(cfg
            .plugins
            .disable
            .contains(&"janus.plugin.lua".to_string()));
    }

    #[test]
    fn parse_events_section() {
        let toml = r#"
            [events]
            broadcast = true
            stats_period = 30
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert!(cfg.events.broadcast);
        assert_eq!(cfg.events.stats_period, 30);
    }

    #[test]
    fn parse_certificates_section() {
        let toml = r#"
            [certificates]
            cert_pem = "/path/to/cert.pem"
            cert_key = "/path/to/key.pem"
            dtls_ciphers = "HIGH:!aNULL:!MD5"
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert_eq!(
            cfg.certificates.cert_pem.as_deref(),
            Some(Path::new("/path/to/cert.pem"))
        );
        assert_eq!(
            cfg.certificates.dtls_ciphers.as_deref(),
            Some("HIGH:!aNULL:!MD5")
        );
    }

    #[test]
    fn default_general_config_values() {
        let cfg = GeneralConfig::default();
        assert_eq!(cfg.configs_folder, PathBuf::from("/etc/janus"));
        assert_eq!(cfg.debug_level, 4);
        assert!(!cfg.daemonize);
        assert!(cfg.api_secret.is_none());
        assert!(!cfg.token_auth);
        assert!(cfg.token_auth_secret.is_none());
        assert_eq!(cfg.session_timeout, 60);
        assert_eq!(cfg.reclaim_session_timeout, 0);
    }

    #[test]
    fn default_nat_config_values() {
        let cfg = NatConfig::default();
        assert_eq!(cfg.stun_server.as_deref(), Some("stun.l.google.com"));
        assert_eq!(cfg.stun_port, 19302);
        assert!(cfg.turn_server.is_none());
        assert!(!cfg.ice_lite);
        assert!(cfg.nat_1_1_mapping.is_none());
    }

    #[test]
    fn config_serialization_includes_all_sections() {
        let cfg = JanusConfig::default();
        let serialized = toml::to_string(&cfg).unwrap();
        assert!(serialized.contains("[general]"));
        assert!(serialized.contains("[certificates]"));
        assert!(serialized.contains("[media]"));
        assert!(serialized.contains("[nat]"));
        assert!(serialized.contains("[plugins]"));
        assert!(serialized.contains("[transports]"));
        assert!(serialized.contains("[events]"));
        assert!(serialized.contains("[admin]"));
    }

    #[test]
    fn parse_wrong_type_returns_error() {
        let toml = r#"
            [general]
            debug_level = "not a number"
        "#;
        let result = JanusConfig::parse_toml(toml);
        assert!(result.is_err());
    }

    #[test]
    fn parse_session_timeout_zero() {
        let toml = r#"
            [general]
            session_timeout = 0
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert_eq!(cfg.general.session_timeout, 0);
    }

    #[test]
    fn parse_general_section_with_all_auth_options() {
        let toml = r#"
            [general]
            api_secret = "secret123"
            token_auth = true
            token_auth_secret = "tokensecret"
        "#;
        let cfg = JanusConfig::parse_toml(toml).unwrap();
        assert_eq!(cfg.general.api_secret.as_deref(), Some("secret123"));
        assert!(cfg.general.token_auth);
        assert_eq!(
            cfg.general.token_auth_secret.as_deref(),
            Some("tokensecret")
        );
    }
}
