//! Which tenant took which provider-native snapshot.
//!
//! A native snapshot has no digest or per-tenant registry repository to scope
//! it, so the broker records each one it hands out and restores only what the
//! requesting tenant recorded.

mod constants;
mod in_memory_native_snapshot_store;
mod native_snapshot_store;
mod nats_kv_native_snapshot_store;
mod owned_snapshot;

pub use in_memory_native_snapshot_store::InMemoryNativeSnapshotStore;
pub use native_snapshot_store::NativeSnapshotStore;
pub use nats_kv_native_snapshot_store::NatsKvNativeSnapshotStore;

#[cfg(test)]
mod tests;
