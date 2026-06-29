//! The storage seam shared by the schedules projectors.
//!
//! Each projector under [`crate::projections`] folds the schedule event stream
//! into the `projections.v1.ScheduleProjection` proto; this trait is where that
//! proto is read from and written to. The [`crate::projections::schedules`]
//! projector writes it to NATS JetStream KV directly; the
//! [`crate::projections::postgres`] projector writes it through this trait into a
//! Postgres table. The read model a caller sees is identical regardless of which
//! projector served it; only the storage and key scheme differ.

use std::collections::HashSet;

use crate::{error::SchedulerError, projections_v1};

/// Storage for the schedules read model: point/list reads, upserts, deletes, the
/// catch-up reconcile, and the catch-up checkpoint.
///
/// All methods work in raw schedule ids and the stored view proto. The catch-up
/// rebuild replays the whole event log from empty, so [`Self::reconcile`] receives
/// the authoritative set of ids that should exist and the backend removes the
/// rest.
///
/// Callers are generic over the store (static dispatch), so plain `async fn` is
/// used rather than boxed, dyn-compatible futures.
#[allow(async_fn_in_trait)]
pub trait SchedulesProjectionStore: Send + Sync {
    /// Reads the stored view for one schedule, or `None` if it is absent.
    async fn get_view(&self, schedule_id: &str) -> Result<Option<projections_v1::ScheduleProjection>, SchedulerError>;

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
