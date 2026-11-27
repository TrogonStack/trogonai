//! Operator-controlled placement fences on individual workers.
//!
//! A fence tells the broker to stop placing new leases on one worker while
//! the leases it already holds carry on. It lives in a NATS KV bucket so an
//! operator can set it with the admin tool without talking to
//! the broker, and every broker converges on it within one refresh interval.
//!
//! - [`WorkerPlacementState::Cordoned`]: no new placements. Nothing else
//!   changes; the operator decides what happens to the leases.
//! - [`WorkerPlacementState::Draining`]: no new placements, and the operator
//!   intends the worker to empty. Placement treats it exactly like a cordon;
//!   the difference is the intent the admin tool reports on. Migrating the
//!   remaining leases away is not done by the broker yet.

mod constants;
mod drain_report;
mod fence_key;
mod fence_record;
mod fence_refresh;
mod fence_view;
mod in_memory_worker_fence_store;
mod nats_kv_worker_fence_store;
mod worker_fence_store;
mod worker_placement_state;

pub use constants::{FENCE_REFRESH_INTERVAL, WORKER_FENCE_BUCKET};
pub use drain_report::{DrainReport, drain_report};
pub use fence_key::fence_kv_key;
pub use fence_record::WorkerFence;
pub use fence_refresh::spawn_fence_refresh;
pub use fence_view::FenceView;
pub use in_memory_worker_fence_store::InMemoryWorkerFenceStore;
pub use nats_kv_worker_fence_store::NatsKvWorkerFenceStore;
pub use worker_fence_store::WorkerFenceStore;
pub use worker_placement_state::WorkerPlacementState;

#[cfg(test)]
mod tests;
