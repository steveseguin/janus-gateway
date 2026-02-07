//! WHIP ingest and WHEP egress for Janus Gateway.
//!
//! Implements RFC 9725 (WHIP) and draft-ietf-wish-whep (WHEP) as axum routes
//! that can be merged into the HTTP transport. WHIP allows clients to publish
//! media via a simple HTTP POST of an SDP offer. WHEP allows clients to
//! subscribe to published streams the same way.

pub mod fanout;
pub mod handlers;
pub mod link_headers;
pub mod sdp_fragment;
pub mod state;
pub mod whip_client;

use axum::{
    routing::{delete, get, options, patch, post},
    Router,
};
use state::WhipWhepState;

/// Build the WHIP/WHEP axum router.
///
/// Mount this alongside the Janus API router on the HTTP transport.
pub fn whip_whep_router(state: WhipWhepState) -> Router {
    Router::new()
        // WHIP: publish
        .route("/whip", post(handlers::whip_offer))
        .route("/whip", options(handlers::options_whip))
        // WHEP: subscribe to a publisher
        .route("/whep/:publisher_id", post(handlers::whep_offer))
        .route("/whep/:publisher_id", options(handlers::options_whep))
        // WHIP-out: relay to remote WHIP endpoints
        .route("/whip-out/:publisher_id", post(handlers::whip_out_add))
        .route("/whip-out/:publisher_id", get(handlers::whip_out_list))
        // Resource management (trickle, ICE restart, delete)
        .route("/resource/:id", patch(handlers::patch_resource))
        .route("/resource/:id", delete(handlers::delete_resource))
        .route("/resource/:id", options(handlers::options_resource))
        .with_state(state)
}
