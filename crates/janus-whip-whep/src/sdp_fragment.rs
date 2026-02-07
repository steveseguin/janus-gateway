//! Parse and generate `application/trickle-ice-sdpfrag` SDP fragments.
//!
//! Used for ICE trickle (PATCH with candidates) and ICE restart responses.

/// Parsed ICE candidates and credentials from an SDP fragment.
#[derive(Debug, Clone, Default)]
pub struct SdpFragment {
    pub ice_ufrag: Option<String>,
    pub ice_pwd: Option<String>,
    pub mid: Option<String>,
    pub candidates: Vec<String>,
}

/// Parse an `application/trickle-ice-sdpfrag` body.
///
/// Extracts `a=ice-ufrag`, `a=ice-pwd`, `a=mid`, and `a=candidate` lines.
pub fn parse_sdp_fragment(body: &str) -> SdpFragment {
    let mut frag = SdpFragment::default();
    for line in body.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("a=ice-ufrag:") {
            frag.ice_ufrag = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("a=ice-pwd:") {
            frag.ice_pwd = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("a=mid:") {
            frag.mid = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("a=candidate:") {
            frag.candidates.push(format!("candidate:{val}"));
        }
    }
    frag
}

/// Generate an SDP fragment for an ICE restart response.
///
/// Returns the fragment body with new ICE credentials.
pub fn generate_ice_restart_fragment(ice_ufrag: &str, ice_pwd: &str, mid: Option<&str>) -> String {
    let mut frag = String::new();
    if let Some(mid) = mid {
        frag.push_str(&format!("a=mid:{mid}\r\n"));
    }
    frag.push_str(&format!("a=ice-ufrag:{ice_ufrag}\r\n"));
    frag.push_str(&format!("a=ice-pwd:{ice_pwd}\r\n"));
    frag
}

/// Content type for trickle ICE SDP fragments.
pub const TRICKLE_ICE_CONTENT_TYPE: &str = "application/trickle-ice-sdpfrag";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_fragment() {
        let frag = parse_sdp_fragment("");
        assert!(frag.ice_ufrag.is_none());
        assert!(frag.ice_pwd.is_none());
        assert!(frag.mid.is_none());
        assert!(frag.candidates.is_empty());
    }

    #[test]
    fn parse_fragment_with_credentials() {
        let body = "a=ice-ufrag:abc123\r\na=ice-pwd:secretpassword\r\n";
        let frag = parse_sdp_fragment(body);
        assert_eq!(frag.ice_ufrag.as_deref(), Some("abc123"));
        assert_eq!(frag.ice_pwd.as_deref(), Some("secretpassword"));
    }

    #[test]
    fn parse_fragment_with_candidates() {
        let body = "\
            a=mid:0\r\n\
            a=candidate:1 1 UDP 2130706431 192.168.1.1 50000 typ host\r\n\
            a=candidate:2 1 UDP 1694498815 203.0.113.1 50000 typ srflx\r\n";
        let frag = parse_sdp_fragment(body);
        assert_eq!(frag.mid.as_deref(), Some("0"));
        assert_eq!(frag.candidates.len(), 2);
        assert!(frag.candidates[0].starts_with("candidate:"));
    }

    #[test]
    fn parse_fragment_with_everything() {
        let body = "\
            a=ice-ufrag:ufrag1\r\n\
            a=ice-pwd:pwd1\r\n\
            a=mid:audio\r\n\
            a=candidate:1 1 UDP 2130706431 10.0.0.1 9999 typ host\r\n";
        let frag = parse_sdp_fragment(body);
        assert_eq!(frag.ice_ufrag.as_deref(), Some("ufrag1"));
        assert_eq!(frag.ice_pwd.as_deref(), Some("pwd1"));
        assert_eq!(frag.mid.as_deref(), Some("audio"));
        assert_eq!(frag.candidates.len(), 1);
    }

    #[test]
    fn parse_fragment_ignores_unknown_lines() {
        let body = "v=0\r\na=ice-ufrag:test\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n";
        let frag = parse_sdp_fragment(body);
        assert_eq!(frag.ice_ufrag.as_deref(), Some("test"));
        assert!(frag.candidates.is_empty());
    }

    #[test]
    fn generate_fragment_with_mid() {
        let frag = generate_ice_restart_fragment("newufrag", "newpwd", Some("0"));
        assert!(frag.contains("a=mid:0\r\n"));
        assert!(frag.contains("a=ice-ufrag:newufrag\r\n"));
        assert!(frag.contains("a=ice-pwd:newpwd\r\n"));
    }

    #[test]
    fn generate_fragment_without_mid() {
        let frag = generate_ice_restart_fragment("u", "p", None);
        assert!(!frag.contains("a=mid:"));
        assert!(frag.contains("a=ice-ufrag:u\r\n"));
        assert!(frag.contains("a=ice-pwd:p\r\n"));
    }

    #[test]
    fn trickle_content_type_constant() {
        assert_eq!(TRICKLE_ICE_CONTENT_TYPE, "application/trickle-ice-sdpfrag");
    }

    #[test]
    fn parse_roundtrip() {
        let generated = generate_ice_restart_fragment("uf", "pw", Some("1"));
        let parsed = parse_sdp_fragment(&generated);
        assert_eq!(parsed.ice_ufrag.as_deref(), Some("uf"));
        assert_eq!(parsed.ice_pwd.as_deref(), Some("pw"));
        assert_eq!(parsed.mid.as_deref(), Some("1"));
    }
}
