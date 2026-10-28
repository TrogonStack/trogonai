//! Query entry points for the alternative projection backends (e.g. Postgres),
//! reading from the Postgres table. The default NATS read-model
//! queries remain at the crate root ([`crate::get_schedule`],
//! [`crate::list_schedules`]).

pub use crate::queries::projection::{get_schedule, list_schedules};
