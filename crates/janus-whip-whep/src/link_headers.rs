//! RFC 8288 Link header generation for STUN/TURN ICE servers.
//!
//! Per RFC 9725 Section 4.1, WHIP endpoints return Link headers
//! advertising available ICE servers.

use janus_core::config::NatConfig;

/// A structured ICE server entry.
#[derive(Debug, Clone)]
pub struct IceServer {
    /// URI(s) like "stun:host:port" or "turn:host:port?transport=udp".
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

/// Build ICE server entries from NatConfig.
pub fn ice_servers_from_nat(nat: &NatConfig) -> Vec<IceServer> {
    let mut servers = Vec::new();

    if let Some(ref stun) = nat.stun_server {
        servers.push(IceServer {
            urls: vec![format!("stun:{stun}:{}", nat.stun_port)],
            username: None,
            credential: None,
        });
    }

    if let Some(ref turn) = nat.turn_server {
        servers.push(IceServer {
            urls: vec![format!("turn:{turn}:{}?transport=udp", nat.turn_port)],
            username: nat.turn_user.clone(),
            credential: nat.turn_pwd.clone(),
        });
    }

    servers
}

/// Generate RFC 8288 Link header values for the given ICE servers.
///
/// Returns a vector of individual Link header values that should each
/// be set as a separate `Link` header.
pub fn link_header_values(servers: &[IceServer]) -> Vec<String> {
    let mut values = Vec::new();
    for server in servers {
        for url in &server.urls {
            let mut link = format!("<{url}>; rel=\"ice-server\"");
            if let Some(ref user) = server.username {
                link.push_str(&format!("; username=\"{user}\""));
            }
            if let Some(ref cred) = server.credential {
                link.push_str(&format!(
                    "; credential=\"{cred}\"; credential-type=\"password\""
                ));
            }
            values.push(link);
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_nat() -> NatConfig {
        NatConfig {
            stun_server: None,
            stun_port: 3478,
            ..NatConfig::default()
        }
    }

    #[test]
    fn no_servers_no_links() {
        let nat = empty_nat();
        let servers = ice_servers_from_nat(&nat);
        assert!(servers.is_empty());
        assert!(link_header_values(&servers).is_empty());
    }

    #[test]
    fn stun_only() {
        let mut nat = NatConfig::default();
        nat.stun_server = Some("stun.l.google.com".into());
        nat.stun_port = 19302;
        let servers = ice_servers_from_nat(&nat);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls[0], "stun:stun.l.google.com:19302");
        assert!(servers[0].username.is_none());

        let links = link_header_values(&servers);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0],
            "<stun:stun.l.google.com:19302>; rel=\"ice-server\""
        );
    }

    #[test]
    fn turn_with_credentials() {
        let mut nat = empty_nat();
        nat.turn_server = Some("turn.example.com".into());
        nat.turn_port = 3478;
        nat.turn_user = Some("user1".into());
        nat.turn_pwd = Some("pass1".into());
        let servers = ice_servers_from_nat(&nat);
        assert_eq!(servers.len(), 1);

        let links = link_header_values(&servers);
        assert_eq!(links.len(), 1);
        assert!(links[0].contains("turn:turn.example.com:3478?transport=udp"));
        assert!(links[0].contains("username=\"user1\""));
        assert!(links[0].contains("credential=\"pass1\""));
        assert!(links[0].contains("credential-type=\"password\""));
    }

    #[test]
    fn stun_and_turn() {
        let mut nat = NatConfig::default();
        nat.stun_server = Some("stun.example.com".into());
        nat.stun_port = 3478;
        nat.turn_server = Some("turn.example.com".into());
        nat.turn_port = 443;
        nat.turn_user = Some("u".into());
        nat.turn_pwd = Some("p".into());
        let servers = ice_servers_from_nat(&nat);
        assert_eq!(servers.len(), 2);

        let links = link_header_values(&servers);
        assert_eq!(links.len(), 2);
        assert!(links[0].contains("stun:"));
        assert!(links[1].contains("turn:"));
    }

    #[test]
    fn turn_without_credentials() {
        let mut nat = empty_nat();
        nat.turn_server = Some("turn.example.com".into());
        let servers = ice_servers_from_nat(&nat);
        let links = link_header_values(&servers);
        assert_eq!(links.len(), 1);
        assert!(!links[0].contains("username"));
        assert!(!links[0].contains("credential"));
    }

    #[test]
    fn link_header_rel_format() {
        let servers = vec![IceServer {
            urls: vec!["stun:s.example.com:3478".into()],
            username: None,
            credential: None,
        }];
        let links = link_header_values(&servers);
        // Must use quoted rel value per RFC 8288
        assert!(links[0].contains("rel=\"ice-server\""));
    }

    #[test]
    fn multiple_urls_per_server() {
        let servers = vec![IceServer {
            urls: vec![
                "turn:a.example.com:3478?transport=udp".into(),
                "turn:a.example.com:443?transport=tcp".into(),
            ],
            username: Some("u".into()),
            credential: Some("p".into()),
        }];
        let links = link_header_values(&servers);
        assert_eq!(links.len(), 2);
    }
}
