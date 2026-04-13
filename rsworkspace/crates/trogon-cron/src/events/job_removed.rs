use super::JobEvent;

pub(super) fn new(id: impl Into<String>) -> JobEvent {
    JobEvent::JobRemoved { id: id.into() }
}

pub(super) fn job_id(id: &str) -> &str {
    id
}
