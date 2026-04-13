use crate::config::JobEnabledState;

use super::JobEvent;

pub(super) fn new(id: impl Into<String>, state: JobEnabledState) -> JobEvent {
    JobEvent::JobStateChanged {
        id: id.into(),
        state,
    }
}

pub(super) fn job_id(id: &str) -> &str {
    id
}
