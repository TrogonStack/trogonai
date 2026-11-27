use std::time::Duration;

use trogon_sandbox_file_ops::Tracking;

/// Watches live per lease at once; matches the docker worker.
pub const MAX_WATCHES_PER_LEASE: usize = Tracking::MAX_WATCHES_PER_LEASE;

/// Events a watch buffers before the oldest are dropped; matches the docker worker.
pub const WATCH_BUFFER_EVENTS: usize = Tracking::WATCH_BUFFER_EVENTS;

/// A watch nobody polled for this long is removed.
pub const WATCH_IDLE_EXPIRY: Duration = Tracking::WATCH_IDLE_EXPIRY;

/// How often a parked poll asks envd for new events.
pub(super) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The shortest poll interval a caller may ask for.
pub(super) const MIN_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// How long releasing a lease waits for envd to drop its watchers.
pub(super) const RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

/// The user envd calls run as when the request names none.
pub(super) const DEFAULT_USER: &str = "user";

/// Prefix of the watch ids this worker hands out.
pub(super) const WATCH_ID_PREFIX: &str = "w-";
