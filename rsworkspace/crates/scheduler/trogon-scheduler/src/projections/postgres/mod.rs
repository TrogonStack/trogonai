//! The Postgres schedules projector.
//!
//! A sibling of the NATS KV projector ([`crate::projections::schedules`]): it folds
//! the same schedule event stream into the same `projections.v1.ScheduleProjection`
//! read model, but persists it in a normalized Postgres table.
//!
//! - [`store`]: `PostgresSchedulesProjection`, the table-backed store.
//! - [`projector`]: `SchedulesProjector`, which drives the store from the stream.
//! - [`postgres_url`]: `PostgresUrl`, the connection settings, which verify the
//!   server certificate unless TLS is disabled explicitly with `sslmode=disable`.
//!
//! The store and projector reach a live database, so they are integration-tested
//! (testcontainers) rather than unit-covered, and compiled only behind the
//! `postgres` feature.

mod postgres_tls;
mod postgres_url;
mod projector;
mod store;

pub use postgres_tls::{PostgresTls, PostgresTlsError};
pub use postgres_url::{PostgresUrl, PostgresUrlError};
pub use projector::SchedulesProjector;
pub use store::PostgresSchedulesProjection;
