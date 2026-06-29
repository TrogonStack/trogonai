//! An alternative schedules read-model backend, stored in Postgres.
//!
//! This is additive: the default NATS JetStream KV read model (under
//! [`crate::projections::schedules`] and queried via [`crate::queries`]) is
//! untouched. This module introduces a storage seam, [`SchedulesProjectionStore`],
//! and a Postgres implementation behind the `postgres` feature, plus its own query
//! entry points under [`crate::projections::queries`].
//!
//! The same `projections.v1.ScheduleProjection` proto is the stored value, so the
//! read model a caller sees is identical regardless of backend; only the storage
//! and key scheme differ.

use std::collections::HashSet;

use async_trait::async_trait;

use crate::{error::SchedulerError, projections_v1};

// The Postgres backend reaches a live database, so it is integration-tested
// (testcontainers) rather than unit-covered; exclude it from the coverage build
// the same way the NATS store/query paths are.
#[cfg(all(feature = "postgres", not(coverage)))]
pub mod postgres;

#[cfg(all(feature = "postgres", not(coverage)))]
pub use postgres::PostgresSchedulesProjection;

/// Storage for the schedules read model: point/list reads, upserts, deletes, the
/// catch-up reconcile, and the catch-up checkpoint.
///
/// All methods work in raw schedule ids and the stored view proto. The catch-up
/// rebuild replays the whole event log from empty, so [`Self::reconcile`] receives
/// the authoritative set of ids that should exist and the backend removes the
/// rest.
#[async_trait]
pub trait SchedulesProjectionStore: Send + Sync {
    /// Reads the stored view for one schedule, or `None` if it is absent.
    async fn get_view(
        &self,
        schedule_id: &str,
    ) -> Result<Option<projections_v1::ScheduleProjection>, SchedulerError>;

    /// Reads every stored view. A single unreadable row must not suppress the rest:
    /// a backend skips it with a warning rather than failing the listing.
    async fn list_views(&self) -> Result<Vec<projections_v1::ScheduleProjection>, SchedulerError>;

    /// Inserts or replaces the stored view for `view.schedule_id`.
    async fn upsert_view(&self, view: &projections_v1::ScheduleProjection) -> Result<(), SchedulerError>;

    /// Removes the stored view for a schedule. Deleting an absent schedule is a
    /// no-op so the projection's delete-on-removal stays idempotent.
    async fn delete_view(&self, schedule_id: &str) -> Result<(), SchedulerError>;

    /// Deletes every stored view whose schedule id is not in `live_ids`.
    ///
    /// Called only after a full catch-up rebuild, where `live_ids` is the
    /// authoritative set of schedules the fold produced. It relies on the
    /// single-active-writer invariant (see the projection module docs).
    async fn reconcile(&self, live_ids: &HashSet<String>) -> Result<(), SchedulerError>;

    /// Reads the catch-up checkpoint (the last fully folded stream sequence), or
    /// `0` when unset or unreadable so a fresh rebuild starts from the beginning.
    async fn read_checkpoint(&self) -> Result<u64, SchedulerError>;

    /// Persists the catch-up checkpoint.
    async fn write_checkpoint(&self, sequence: u64) -> Result<(), SchedulerError>;
}
