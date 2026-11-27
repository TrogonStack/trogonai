use std::time::Duration;

pub(super) const SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(25);
pub(super) const EXIT_GRACE: Duration = Duration::from_secs(5);
