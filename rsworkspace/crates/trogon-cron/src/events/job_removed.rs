use std::collections::BTreeMap;

use crate::{
    config::{JobSpec, VersionedJobSpec},
    error::CronError,
};

use super::{JobEvent, ProjectionChange};

pub(super) fn new(id: impl Into<String>) -> JobEvent {
    JobEvent::JobRemoved { id: id.into() }
}

pub(super) fn job_id(id: &str) -> &str {
    id
}

pub(super) fn apply_to_state(
    id: &str,
    jobs: &mut BTreeMap<String, JobSpec>,
) -> Result<ProjectionChange, CronError> {
    jobs.remove(id);
    Ok(ProjectionChange::Delete(id.to_string()))
}

pub(super) fn apply_to_versioned_state(
    id: &str,
    jobs: &mut BTreeMap<String, VersionedJobSpec>,
) -> Result<(), CronError> {
    jobs.remove(id);
    Ok(())
}
