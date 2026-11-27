use std::time::Duration;

/// Native options this worker accepts under `native.e2b`.
pub const NATIVE_OPTION_KEYS: &[&str] = &["auto_pause"];

/// The native option that asks E2B to pause a sandbox instead of killing it when its lifetime ends.
pub(crate) const AUTO_PAUSE_OPTION: &str = "auto_pause";

/// What E2B reports for a sandbox that runs.
pub(crate) const STATE_RUNNING: &str = "running";

/// What E2B reports for a sandbox that is paused.
pub(crate) const STATE_PAUSED: &str = "paused";

/// The manifest layout this worker writes for a new snapshot.
pub(crate) const SNAPSHOT_MANIFEST_VERSION: u32 = 3;

/// Slack added on top of two API round trips before an owned cleanup claim may be taken over.
pub(crate) const CLEANUP_CLAIM_SLACK: Duration = Duration::from_secs(5);

/// How many API round trips a cleanup claim must outlive.
pub(crate) const CLEANUP_CLAIM_ROUND_TRIPS: u32 = 2;
