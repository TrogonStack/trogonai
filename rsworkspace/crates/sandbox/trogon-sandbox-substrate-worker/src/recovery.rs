//! Reconciling this worker's view with the control plane's.
//!
//! Two jobs that read the same two sources and disagree about which one is authoritative.
//! Startup recovery trusts the index and asks the control plane whether each entry still
//! stands. The reaper trusts the control plane and asks the index whether each actor is
//! claimed. Everything either one destroys is something both agree is not backing a lease.

mod adopt;
mod atespace_of;
mod constants;
mod lease_id_of;
mod prune;
mod reap;
mod reap_loop;
mod recover;
mod recovery_error;
mod recovery_report;

pub use constants::REAP_INTERVAL;
pub use lease_id_of::lease_id_of;
pub use reap::reap;
pub use reap_loop::reap_loop;
pub use recover::recover;
pub use recovery_error::RecoveryError;
pub use recovery_report::RecoveryReport;

#[cfg(test)]
mod tests;
