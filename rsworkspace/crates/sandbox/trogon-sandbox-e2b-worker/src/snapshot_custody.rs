//! Durable ownership of in-flight and orphaned native snapshots, so cleanup survives a crash.

mod clock;
mod clock_error;
mod constants;
mod custody_error;
mod phase;
mod record;
mod store;
mod stored;
#[cfg(test)]
mod tests;

pub(crate) use clock::{after_ms, now_ms};
pub use clock_error::SnapshotClockError;
pub(crate) use constants::MAX_AUTOMATIC_ATTEMPTS;
pub use custody_error::SnapshotCustodyError;
pub(crate) use phase::CustodyPhase;
pub(crate) use record::CustodyRecord;
pub(crate) use store::SnapshotCustody;
pub(crate) use stored::StoredCustody;
