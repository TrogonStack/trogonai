use std::time::Duration;

/// How often the worker beats until the service advises another interval.
pub const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 20_000;

/// The shortest interval the service may advise.
pub const HEARTBEAT_INTERVAL_FLOOR_MS: u64 = 10_000;

/// The longest interval the service may advise.
pub const HEARTBEAT_INTERVAL_CEILING_MS: u64 = 300_000;

/// The longest a single heartbeat may wait for its answer.
pub const HEARTBEAT_TIMEOUT_CEILING_MS: u64 = 15_000;

/// How much of the interval a heartbeat leaves for the next one to start on time.
pub const HEARTBEAT_TIMEOUT_MARGIN_MS: u64 = 5_000;

/// The shortest a single heartbeat may wait for its answer.
pub const HEARTBEAT_TIMEOUT_FLOOR_MS: u64 = 5_000;

/// Consecutive `401` or `403` answers after which the worker gives up.
pub const AUTH_FAILURES_BEFORE_EXIT: u32 = 10;

/// Consecutive `404` answers after which the worker gives up.
pub const NOT_FOUND_BEFORE_EXIT: u32 = 3;

pub(super) const REGISTER_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) const INIT_ATTEMPTS: u32 = 10;

pub(super) const GET_ATTEMPTS: u32 = 10;

pub(super) const GOODBYE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) const GET_TIMEOUT: Duration = Duration::from_secs(30);

/// `initStateGetOrderingBoundMs` (bridge.mjs offset 1133 region, `dHe`): how long `initialize` waits for the stored
/// record before it registers.
pub(super) const INIT_STATE_GET_ORDERING_BOUND: Duration = Duration::from_secs(10);

pub(super) const INIT_BACKOFF_CEILING_MS: u64 = 30_000;

/// `qHe` (bridge.mjs offset 1019297): the metadata keys that are not sent again when the stored record holds them.
pub(super) const RECORDED_KEYS: [&str; 6] = [
    "context_usage",
    "cross_session_inbound",
    "is_ultraplan_mode",
    "last_served_model",
    "permission_mode",
    "rate_limit_info",
];

pub(super) const SESSION_TICKET_HEADER: &str = "x-ccr-ticket-session";

pub(super) const TICKET_HEADERS: [&str; 3] = ["x-ccr-ticket-rev", SESSION_TICKET_HEADER, "x-ccr-ticket-seq"];
