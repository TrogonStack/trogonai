/// The reconnect delay of the first failure (`W4e`).
pub const RECONNECT_BASE_MS: u64 = 1_000;

/// The longest reconnect delay before jitter (`j4e`).
pub const RECONNECT_CAP_MS: u64 = 30_000;

/// The jitter ratio of the reconnect delay (`symmetric`, `0.25`).
pub const RECONNECT_JITTER_RATIO: f64 = 0.25;

/// The statuses that close the session instead of being retried (`t$e`).
pub const PERMANENT_STREAM_STATUSES: [u16; 3] = [401, 403, 404];

/// The silence the service keeps between frames, in seconds, when it advertises nothing else (`bC`).
pub const KEEPALIVE_SECONDS: u64 = 15;

/// The grace added to the silence before the stream is called dead (`V4e`).
pub const LIVENESS_GRACE_MS: u64 = 30_000;

/// How late the liveness check may run before the process is taken to have been paused or asleep (`s9`).
pub const SLEEP_LATENESS_MS: u64 = 10_000;

/// The `event_type` values only the service may send: a payload of such a type under another `event_type` is vetoed.
pub(super) const SERVER_ONLY_TYPES: [&str; 3] = ["workflow_launch", "queued_notification", "session_notice"];

/// The most sequence numbers remembered for the duplicate log before the old ones are dropped.
pub(super) const SEEN_LIMIT: usize = 1_000;

/// How far below the highest sequence number a remembered one may sit when the old ones are dropped.
pub(super) const SEEN_WINDOW: i64 = 200;
