use std::time::Duration;

/// How often the resume loop asks whether the actor has reached `RUNNING`.
pub(super) const RESUME_POLL_INTERVAL: Duration = Duration::from_millis(250);
