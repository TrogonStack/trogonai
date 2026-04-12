use std::collections::BTreeMap;

use crate::{
    config::{JobEnabledState, JobSpec, VersionedJobSpec},
    error::CronError,
};

use super::{JobEvent, ProjectionChange, projection_invariant_error};

pub(super) fn new(id: impl Into<String>, state: JobEnabledState) -> JobEvent {
    JobEvent::JobStateChanged {
        id: id.into(),
        state,
    }
}

pub(super) fn job_id(id: &str) -> &str {
    id
}

pub(super) fn apply_to_state(
    id: &str,
    state: JobEnabledState,
    jobs: &mut BTreeMap<String, JobSpec>,
) -> Result<ProjectionChange, CronError> {
    let spec = jobs
        .get_mut(id)
        .ok_or_else(|| projection_invariant_error("missing job for state change", id))?;
    spec.state = state;
    Ok(ProjectionChange::Upsert(spec.clone()))
}

pub(super) fn apply_to_versioned_state(
    id: &str,
    state: JobEnabledState,
    jobs: &mut BTreeMap<String, VersionedJobSpec>,
    version: u64,
) -> Result<(), CronError> {
    let job = jobs
        .get_mut(id)
        .ok_or_else(|| projection_invariant_error("missing versioned job for state change", id))?;
    job.version = version;
    job.spec.state = state;
    Ok(())
}
