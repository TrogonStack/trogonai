//! Projects current [`crate::KillSwitchState`] into a NATS JetStream Key/Value
//! bucket, following the same shape as `a2a-nats`'s `KvCatalogStore`
//! (`crates/a2a-nats/src/catalog/store.rs`): one bucket, one key per entity
//! id, tombstone-aware create-vs-update, bounded conflict retry.
//!
//! This module has no consumer loop that folds `KILL_SWITCH_EVENTS` into the
//! bucket; it only provides the KV read/write primitive. Wiring a JetStream
//! consumer to call [`store::KillSwitchProjectionStore::put`] on every
//! `AgentKilled`/`AgentRevived` event is enforcement-adjacent plumbing left
//! to `docs/proposals/kill-switch-enforcement.md`, matching this crate's
//! domain-core-first scope.

mod kill_status_wire;
mod kv_bucket;
mod store;

pub use kill_status_wire::KillStatusWireError;
pub use kv_bucket::{KILL_SWITCH_STATE, kill_switch_state_bucket_config};
pub use store::{BoxedKvError, KillSwitchProjectionError, KillSwitchProjectionStore};
