//! Per-request anomaly feature vectors for downstream scoring.
//!
//! Wired from `gateway::handle_ingress_inner` after hierarchical policy + SpiceDB
//! authorization (immediately before mesh egress minting): derive [`AnomalyFeatures`]
//! from audit identity fields (`agent_id`, `purpose`, `tenant`, `act_chain`, backend
//! `target`) and call [`AnomalyEmit::emit`] best-effort without blocking egress.

mod build;
mod emitter;
mod errors;
mod fake;
mod features;
mod novelty;
mod rate;

pub use build::{AnomalyIngressContext, AnomalyIngressSnapshot};
pub use emitter::{AnomalyEmit, AnomalyEmitter, subject_for_tenant};
pub use fake::FakeAnomalyEmitter;
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
