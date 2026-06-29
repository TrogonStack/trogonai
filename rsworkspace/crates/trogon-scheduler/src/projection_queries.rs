//! Query entry points for the alternative projection backends (e.g. Postgres),
//! reading through [`crate::SchedulesProjectionStore`]. The default NATS read-model
//! queries remain at the crate root ([`crate::get_schedule`],
//! [`crate::list_schedules`]).
//!
//! These read through a live backend, so (like the NATS query path) they are
//! integration-tested and excluded from the coverage build.

pub use crate::projections::queries::{get_schedule, list_schedules};
