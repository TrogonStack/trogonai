use std::time::Duration;

/// Retry budget for compare-and-swap snapshot tag mutations.
pub(super) const SNAPSHOT_TAG_MUTATION_MAX_RETRIES: u8 = 3;
/// Most nodes `snapshot_tree` returns; bounds the parent walk against a long
/// or corrupted chain in the snapshot KV bucket.
pub(super) const MAX_SNAPSHOT_TREE_DEPTH: usize = 64;

/// Compare-and-swap attempts one snapshot metadata write makes.
pub(super) const SNAPSHOT_KV_CAS_ATTEMPTS: usize = 16;

/// Deadline for the purge_named round trip to the substrate worker fleet.
/// Unlike `provider_dispatch_timeout`, this never blocks on a cold
/// golden-snapshot build, so it does not need that budget's generous headroom.
pub(super) const PURGE_NAMED_TIMEOUT: Duration = Duration::from_secs(15);

/// Tries a delete-by-id makes at removing the deleted lease's snapshot tag
/// before it reports the failure, and the first pause between them (doubled
/// after each failure). Short on purpose: a retried delete and the reconciler
/// pick up where this leaves off.
pub(super) const PURGE_TAG_DELETE_ATTEMPTS: u32 = 3;
pub(super) const PURGE_TAG_DELETE_BACKOFF: Duration = Duration::from_millis(100);

/// Default age before an unbacked drive claim counts as orphaned. A grant sets
/// its drive claims before it writes the active lease record and may wait out
/// the provider dispatch timeout (600s by default), so the default leaves
/// three times that margin.
pub const DEFAULT_ORPHAN_CLAIM_GRACE: Duration = Duration::from_secs(30 * 60);

/// Most drive claims one orphan sweep examines.
pub(super) const ORPHAN_CLAIM_SCAN_LIMIT: usize = 1024;

/// Most orphaned claims one sweep releases.
pub(super) const ORPHAN_CLAIM_RELEASES_PER_SWEEP: usize = 32;

/// Age at which the reconciler takes over an unfinished delete-by-id purge,
/// long enough that the request which wrote the marker has finished or died.
pub const PENDING_PURGE_RECONCILE_AFTER: Duration = Duration::from_secs(60);

/// Age at which the reconciler treats a pause or resume marker as abandoned.
/// Well past the bus request timeout, so a request still in flight has ended.
pub const FREEZE_IN_FLIGHT_STALE_AFTER: Duration = Duration::from_secs(120);

/// Refusals after which the reconciler stops asking the worker to settle a
/// stalled pause or resume and leaves it for an operator.
pub const FREEZE_SETTLE_MAX_ATTEMPTS: u32 = 8;

pub(super) const FREEZE_SETTLE_BACKOFF_BASE: Duration = Duration::from_secs(30);
pub(super) const FREEZE_SETTLE_BACKOFF_MAX: Duration = Duration::from_secs(3600);

/// Kept below the client's 10s write_stdin request timeout so the broker answers
/// with `Backpressure` before the client gives up on the request.
pub(super) const PROVIDER_WRITE_STDIN_TIMEOUT: Duration = Duration::from_secs(8);

/// Bound on one terminal frame's round trip to the worker. Short, because a
/// keystroke that takes seconds is already a failure the user sees.
pub(super) const PROVIDER_TERMINAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Headroom past a foreground command's own timeout for the worker to kill it
/// and reply. Kept below the client's run_command grace so the broker's answer
/// reaches the client before the client gives up on the request.
pub(super) const PROVIDER_RUN_COMMAND_GRACE: Duration = Duration::from_secs(5);

/// How long the broker waits on a worker starting a detached command. Starting
/// one creates a process and returns its id, but hosted providers (Vercel,
/// Daytona) can take well past the transport's 10s default to answer. Kept
/// below the client's detached start timeout so the broker answers first.
pub(super) const PROVIDER_DETACHED_START_TIMEOUT: Duration = Duration::from_secs(30);

/// Pause between readiness checks of one `wait_sandbox_ready` call.
pub(super) const SANDBOX_READY_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Longest one port probe command may run.
pub(super) const PORT_PROBE_TIMEOUT_SECONDS: u32 = 5;
/// Output ceiling for a probe: it prints at most `MAX_READY_PORTS` numbers.
pub(super) const PORT_PROBE_MAX_OUTPUT_BYTES: u64 = 1024;
/// Exit code of [`PORT_PROBE_SCRIPT`] when the sandbox has neither bash nor nc.
pub(super) const PORT_PROBE_NO_TOOL_EXIT: i32 = 3;
/// Prints each port argument that refuses a loopback connection, one per line.
pub(super) const PORT_PROBE_SCRIPT: &str = r#"if command -v bash >/dev/null 2>&1; then
  for p; do bash -c "exec 3<>/dev/tcp/127.0.0.1/$p" >/dev/null 2>&1 || echo "$p"; done
elif command -v nc >/dev/null 2>&1; then
  for p; do nc -z -w 1 127.0.0.1 "$p" >/dev/null 2>&1 || echo "$p"; done
else
  exit 3
fi"#;

/// How often a retry waits on a launch key whose first launch is in flight.
pub(super) const LAUNCH_KEY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a launch key keeps replaying the first reply. Long enough for a
/// client to retry a reply it lost in transit, short enough that command
/// output does not outlive its use.
pub(super) const LAUNCH_KEY_REPLAY_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Slack past a launch's dispatch timeout before a still-pending key is taken
/// as lost to a broker crash.
pub(super) const LAUNCH_KEY_LOST_GRACE: Duration = Duration::from_secs(30);

/// Largest reply a launch key keeps for replay once the full reply failed to
/// store. Well under the default JetStream payload limit, leaving room for the
/// journal entry's own fields.
pub(super) const LAUNCH_JOURNAL_MAX_REPLY_BYTES: usize = 256 * 1024;

/// How often a retry polls an operation another task is driving.
pub(super) const POLL_EVERY: Duration = Duration::from_millis(50);

/// Extra time, on top of the provider dispatch timeout, a caller waits for an
/// operation that someone else is driving before it is told to poll.
pub(super) const WAIT_SLACK: Duration = Duration::from_secs(5);

/// Longest pause before an operation whose attempt stayed unsettled is retried.
pub(super) const MAX_UNSETTLED_RETRY_AFTER: Duration = Duration::from_secs(60);

/// How long a caller that met a wake in progress waits before asking again.
pub(super) const WAKE_RETRY_AFTER_MS: u64 = 1000;
