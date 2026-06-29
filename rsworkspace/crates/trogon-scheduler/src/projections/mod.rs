//! The schedules read-model projection (a NATS KV view of current schedules,
//! folded from the schedule event stream).
//!
//! # Boundary
//!
//! This module is the **write side** only: it folds `v1` event protos straight
//! into the stored `projections.v1.ScheduleProjection` proto and owns that storage layout
//! (bucket, key scheme, checkpoint — see [`schedules::storage`]). It depends on
//! the event/view protos, the shared error type, and the shared events stream —
//! never on the read model or the queries. Reading the stored proto back out and
//! decoding it into the read-model value objects callers see is the **read side**
//! and lives entirely in [`crate::queries`].
//!
//! # Single active writer
//!
//! This projection must be driven by a single active instance, the same
//! invariant the execution worker requires (see `processor::execution::worker`).
//! The event log itself is safe under concurrent writers — per-subject
//! optimistic concurrency serializes appends — but the KV writes here are
//! read-modify-write without compare-and-swap, and [`catch_up_schedules_read_model`]
//! rebuilds the bucket from the event log. With two instances writing the same
//! bucket, a write can land on stale state, or a boot-time rebuild can overwrite
//! a peer's newer write.
//!
//! Enforce this by deploying the scheduler as a single active instance (a single
//! replica, a `Recreate` rollout, or leader election) — not with a
//! projection-specific lock, since the worker already gates the whole service.
//!
//! ## Safety net for the rolling-restart overlap
//!
//! A rolling deploy briefly runs two instances. Most of the projection degrades
//! gracefully, not corrupts, during that window:
//! - Catch-up early-returns when the checkpoint is current, so a booting instance
//!   does not rebuild (or reconcile) in steady state.
//! - The checkpoint only advances contiguously, so a racing writer can stall it
//!   (forcing a harmless re-fold) but never advance it past an unprojected event.
//! - A live projection failure never fails the durable append and never advances
//!   the checkpoint, so any divergence self-heals on the next clean start.
//!
//! The one step that genuinely depends on the single-writer invariant is the
//! catch-up reconcile: it deletes any current row absent from the freshly folded
//! state, so during an overlap it could delete a row a peer just created. That
//! row is re-created by the peer's next event or restart.
//!
//! The residual is a transient stale (or briefly missing) schedule that resolves
//! on the schedule's next event or the next restart.

mod schedules;

pub(crate) use schedules::storage;
#[cfg(not(coverage))]
pub(crate) use schedules::{catch_up_schedules_read_model, project_appended_events};
