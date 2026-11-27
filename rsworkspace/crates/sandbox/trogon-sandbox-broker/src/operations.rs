//! Durable operations: a record of every create, restore, pause, resume and
//! delete the broker accepted, kept in a KV bucket so a restart or a lost reply never loses the
//! outcome and a retry with the same id replays it.
//!
//! Layout of the bucket (one injective encoding, no character replacement):
//! - `op.<hex tenant>.<operation id>`: the [`trogon_sandbox_contract::OperationRecord`].
//! - `active.<hex tenant>.<operation id>`: marker that exists while the
//!   operation is not terminal. Recovery scans only these.
//! - `target.<hex lease id>`: the operation currently holding the lease, so at
//!   most one operation per lease is in flight.
//!
//! Every write is a compare-and-swap on the KV revision. A running operation
//! carries a claim `(owner, generation)`; a write from an older generation is
//! fenced out.

mod begun;
mod cancellation_intent;
mod claim_token;
mod cleanup;
mod codec;
mod constants;
mod entry;
mod in_memory_operation_store;
mod keys;
mod ledger;
mod ledger_cleanup;
mod ledger_settle;
mod nats_kv_operation_store;
mod operation_store;
mod operation_store_error;
mod operations_error;
mod settled;
mod versioned;

pub use begun::Begun;
pub use claim_token::ClaimToken;
pub(crate) use cleanup::CleanupPhase;
pub use constants::{MAX_ATTEMPTS, OPERATION_RETENTION, OPERATIONS_BUCKET, RETRY_AFTER, STALE_CLAIM_AFTER};
pub use entry::Entry;
pub use in_memory_operation_store::InMemoryOperationStore;
pub use keys::{active_key, fingerprint, op_key, target_key};
pub use ledger::Operations;
pub use nats_kv_operation_store::NatsKvOperationStore;
pub use operation_store::OperationStore;
pub use operation_store_error::OperationStoreError;
pub use operations_error::OperationsError;
pub use settled::Settled;
pub use versioned::Versioned;

#[cfg(test)]
mod tests;
