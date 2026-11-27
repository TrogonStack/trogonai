//! Operator path for a pause or resume the reconciler gave up settling.
//!
//! After [`crate::core::FREEZE_SETTLE_MAX_ATTEMPTS`] refusals the broker stops
//! asking the worker and counts the lease in the
//! `sandbox_broker_unsettled_freezes` gauge. The marker then stays until an
//! operator decides what the worker's state is worth.
mod freeze_admin_error;
mod freeze_resolution;
mod freeze_resolve_outcome;
mod resolve_unsettled_freeze;

pub use freeze_admin_error::FreezeAdminError;
pub use freeze_resolution::FreezeResolution;
pub use freeze_resolve_outcome::FreezeResolveOutcome;
pub use resolve_unsettled_freeze::resolve_unsettled_freeze;

#[cfg(test)]
mod tests;
