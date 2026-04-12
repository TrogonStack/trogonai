use std::collections::BTreeMap;

use crate::{
    config::{JobSpec, VersionedJobSpec},
    error::CronError,
};

use super::{JobEvent, ProjectionChange};

pub(super) fn new(spec: JobSpec) -> JobEvent {
    JobEvent::JobRegistered { spec }
}

pub(super) fn job_id(spec: &JobSpec) -> &str {
    &spec.id
}

pub(super) fn apply_to_state(
    spec: &JobSpec,
    jobs: &mut BTreeMap<String, JobSpec>,
) -> Result<ProjectionChange, CronError> {
    jobs.insert(spec.id.clone(), spec.clone());
    Ok(ProjectionChange::Upsert(spec.clone()))
}

pub(super) fn apply_to_versioned_state(
    spec: &JobSpec,
    jobs: &mut BTreeMap<String, VersionedJobSpec>,
    version: u64,
) -> Result<(), CronError> {
    jobs.insert(
        spec.id.clone(),
        VersionedJobSpec {
            version,
            spec: spec.clone(),
        },
    );
    Ok(())
}
