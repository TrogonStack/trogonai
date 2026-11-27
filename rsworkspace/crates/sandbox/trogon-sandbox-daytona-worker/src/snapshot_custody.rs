//! Durable ownership of native snapshots, so cleanup survives a crash and never touches a snapshot the worker did not
//! take.

mod capture;
mod capture_flow;
mod cleanup;
mod constants;
mod custody_error;
mod record;
mod store;
mod stored;
#[cfg(test)]
mod tests;

pub use custody_error::SnapshotCustodyError;
pub use store::SnapshotCustody;
