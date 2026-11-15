use std::time::Duration;

/// How long a release call waits for its answer when the caller sets nothing.
pub const DEFAULT_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

/// The longest a release call may be told to wait. A caller blocked on a
/// release holds an outbound request open, so a longer wait is refused rather
/// than clamped.
pub const MAX_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest a released value is held after it was fetched, whatever its
/// `valid_until` says (ADR#0049).
pub const RELEASE_HOLD_BOUND: Duration = Duration::from_secs(300);

/// The domain and version tokens of the secrets exchange subjects (ADR#0081
/// Decision 3).
pub(crate) const SUBJECT_DOMAIN: &str = "secrets.v1";

/// The service and method the release call is bound to (ADR 0016).
pub(crate) const SERVICE_NAME: &str = "SecretExchangeService";
pub(crate) const METHOD_NAME: &str = "ResolveForRelease";
