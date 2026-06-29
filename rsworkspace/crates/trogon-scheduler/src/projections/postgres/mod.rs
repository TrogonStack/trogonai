//! The Postgres schedules projector.
//!
//! A sibling of the NATS KV projector ([`crate::projections::schedules`]): it folds
//! the same schedule event stream into the same `projections.v1.ScheduleProjection`
//! read model, but persists it in a normalized Postgres table.
//!
//! - [`store`] — `PostgresSchedulesProjection`, the table-backed store.
//! - [`projector`] — `SchedulesProjector`, which drives the store from the stream.
//!
//! It reaches a live database, so it is integration-tested (testcontainers) rather
//! than unit-covered, and compiled only behind the `postgres` feature.

mod projector;
mod store;

pub use projector::SchedulesProjector;
pub use store::PostgresSchedulesProjection;
