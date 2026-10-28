use super::{ProjectionChange, ScheduleStreamState, projections_v1};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AppliedSchedule {
    Present(Box<projections_v1::ScheduleProjection>),
    Deleted(String),
}

impl From<AppliedSchedule> for ScheduleStreamState {
    fn from(value: AppliedSchedule) -> Self {
        match value {
            AppliedSchedule::Present(view) => Self::Present(view),
            AppliedSchedule::Deleted(id) => Self::Deleted(id),
        }
    }
}

impl From<&AppliedSchedule> for ProjectionChange {
    fn from(value: &AppliedSchedule) -> Self {
        match value {
            AppliedSchedule::Present(view) => Self::Upsert(view.clone()),
            AppliedSchedule::Deleted(id) => Self::Delete(id.clone()),
        }
    }
}
