use std::time::Duration;

pub(super) const FREE_TIMEOUT: Duration = Duration::from_millis(5000);

pub(super) const HINT_BUFFER: Duration = Duration::from_millis(1000);

pub(super) const MAX_FREE: usize = 256;
