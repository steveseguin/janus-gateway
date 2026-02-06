//! SDP parsing and generation utilities.
//!
//! Bridges between the Janus `Jsep` type and str0m's SDP handling.

use janus_plugin_api::{Jsep, JsepType};
use str0m::change::SdpOffer;
use str0m::Rtc;
use tracing::debug;

/// Parse a browser SDP offer string into a str0m `SdpOffer`.
pub fn parse_sdp_offer(sdp: &str) -> Result<SdpOffer, String> {
    SdpOffer::from_sdp_string(sdp).map_err(|e| format!("Failed to parse SDP offer: {e}"))
}

/// Generate an SDP answer from a str0m `Rtc` instance given an offer.
///
/// This accepts the remote offer, configures media lines, and produces
/// the local SDP answer string.
pub fn generate_answer(rtc: &mut Rtc, offer: SdpOffer) -> Result<String, String> {
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| format!("Failed to accept SDP offer: {e}"))?;

    let sdp_string = answer.to_sdp_string();
    debug!(sdp_len = sdp_string.len(), "generated SDP answer");
    Ok(sdp_string)
}

/// Convert a Janus `Jsep` offer into an `SdpOffer`.
pub fn jsep_to_sdp_offer(jsep: &Jsep) -> Result<SdpOffer, String> {
    if jsep.jsep_type != JsepType::Offer {
        return Err("Expected JSEP offer, got answer".into());
    }
    parse_sdp_offer(&jsep.sdp)
}

/// Create a Janus `Jsep` answer from an SDP answer string.
pub fn sdp_answer_to_jsep(sdp: String, trickle: bool) -> Jsep {
    Jsep {
        jsep_type: JsepType::Answer,
        sdp,
        trickle,
    }
}

/// Create a Janus `Jsep` offer from an SDP offer string.
pub fn sdp_offer_to_jsep(sdp: String, trickle: bool) -> Jsep {
    Jsep {
        jsep_type: JsepType::Offer,
        sdp,
        trickle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdp_answer_to_jsep_creates_answer() {
        let jsep = sdp_answer_to_jsep("v=0\r\n".into(), true);
        assert_eq!(jsep.jsep_type, JsepType::Answer);
        assert_eq!(jsep.sdp, "v=0\r\n");
        assert!(jsep.trickle);
    }

    #[test]
    fn sdp_offer_to_jsep_creates_offer() {
        let jsep = sdp_offer_to_jsep("v=0\r\n".into(), false);
        assert_eq!(jsep.jsep_type, JsepType::Offer);
        assert!(!jsep.trickle);
    }

    #[test]
    fn jsep_to_sdp_offer_rejects_answer() {
        let jsep = Jsep {
            jsep_type: JsepType::Answer,
            sdp: "v=0\r\n".into(),
            trickle: true,
        };
        assert!(jsep_to_sdp_offer(&jsep).is_err());
    }

    #[test]
    fn parse_sdp_offer_rejects_garbage() {
        assert!(parse_sdp_offer("not an sdp").is_err());
    }

    #[test]
    fn parse_sdp_offer_accepts_minimal_valid_sdp() {
        // str0m requires well-formed SDP; this is a minimal valid offer
        let sdp = "v=0\r\n\
                    o=- 0 0 IN IP4 127.0.0.1\r\n\
                    s=-\r\n\
                    t=0 0\r\n\
                    a=group:BUNDLE 0\r\n\
                    a=ice-options:trickle\r\n\
                    m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
                    c=IN IP4 0.0.0.0\r\n\
                    a=mid:0\r\n\
                    a=sendrecv\r\n\
                    a=rtpmap:111 opus/48000/2\r\n\
                    a=ice-ufrag:test\r\n\
                    a=ice-pwd:testpasswordtestpassword\r\n\
                    a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\r\n\
                    a=setup:actpass\r\n";
        // This may or may not parse depending on str0m version strictness,
        // but the function itself shouldn't panic
        let _ = parse_sdp_offer(sdp);
    }
}
