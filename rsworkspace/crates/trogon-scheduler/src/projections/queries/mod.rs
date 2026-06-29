//! Query entry points for the alternative projection backends (those behind
//! [`crate::projections::backend::SchedulesProjectionStore`]).
//!
//! These mirror the NATS read-model queries in [`crate::queries`] but read through
//! the storage trait instead of a concrete NATS bucket, so they work over any
//! backend (today: Postgres). They reuse the same command value objects
//! ([`crate::GetSchedule`], [`crate::ListSchedules`]) and decode into the same
//! [`crate::Schedule`] read model, so a caller cannot tell which backend served
//! the result.

mod get;
mod list;

pub use get::run as get_schedule;
pub use list::run as list_schedules;
