mod decode;
mod get;
mod list;
mod schedule_id;

/// The read-model value objects callers receive. They belong to the read side:
/// the projection stores a `projections.v1.ScheduleProjection` protobuf, and these
/// queries decode it into these types (see [`decode`]).
pub(crate) mod read_model;

#[cfg(not(coverage))]
pub use get::run as get_schedule;
pub use get::{GetSchedule, GetSchedule as GetScheduleCommand};
#[cfg(not(coverage))]
pub use list::run as list_schedules;
pub use list::{ListSchedules, ListSchedules as ListSchedulesCommand};
pub use schedule_id::{ScheduleId, ScheduleIdError};
