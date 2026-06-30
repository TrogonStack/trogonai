mod decode;
mod get_schedule;
mod list_schedules;
mod schedule_id;

/// Query entry points over the Postgres projection ([`crate::PostgresSchedulesProjection`]),
/// as opposed to the NATS KV queries above.
#[cfg(all(feature = "postgres", not(coverage)))]
pub mod projection;

/// The read-model value objects callers receive. They belong to the read side:
/// the projection stores a `projections.v1.ScheduleProjection` protobuf, and these
/// queries decode it into these types (see [`decode`]).
pub(crate) mod read_model;

#[cfg(not(coverage))]
pub use get_schedule::run as get_schedule;
pub use get_schedule::{GetSchedule, GetSchedule as GetScheduleCommand};
#[cfg(not(coverage))]
pub use list_schedules::run as list_schedules;
pub use list_schedules::{ListSchedules, ListSchedules as ListSchedulesCommand};
pub use schedule_id::{ScheduleId, ScheduleIdError};
