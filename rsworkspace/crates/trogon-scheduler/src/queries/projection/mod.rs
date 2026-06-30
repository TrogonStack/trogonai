//! Query entry points for the Postgres projection ([`crate::PostgresSchedulesProjection`]).
//!
//! These mirror the NATS read-model queries in [`crate::queries`] but read from the
//! Postgres table instead of a NATS bucket. They reuse the same command value
//! objects ([`crate::GetSchedule`], [`crate::ListSchedules`]) and decode into the
//! same [`crate::Schedule`] read model, so a caller cannot tell which projection
//! served the result.

mod get_schedule;
mod list_schedules;

pub use get_schedule::run as get_schedule;
pub use list_schedules::run as list_schedules;
