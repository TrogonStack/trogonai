//! Per-request anomaly feature vectors for downstream scoring.
//!
//! Future wiring: after hierarchical policy + SpiceDB authorization succeed in
//! `gateway::handle_ingress_inner` (immediately before mesh egress minting around
//! `gateway.rs` line 476), derive [`AnomalyFeatures`] from the audit identity fields
//! (`agent_id`, `purpose`, `tenant`, `act_chain`, backend `target`) and call
//! [`AnomalyEmitter::emit`] best-effort without blocking egress.

mod emitter;
mod errors;
mod features;
mod novelty;
mod rate;

pub use emitter::{AnomalyEmitter, subject_for_tenant};
pub use errors::AnomalyError;
pub use features::AnomalyFeatures;
pub use novelty::NoveltyTracker;
pub use rate::RateTracker;

use std::time::{SystemTime, UNIX_EPOCH};

#[must_use]
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
