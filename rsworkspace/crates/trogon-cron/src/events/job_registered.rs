use crate::config::JobSpec;

use super::JobEvent;

pub(super) fn new(spec: JobSpec) -> JobEvent {
    JobEvent::JobRegistered { spec }
}

pub(super) fn job_id(spec: &JobSpec) -> &str {
    &spec.id
}
