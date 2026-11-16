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

/// The most a release's cache expiry is pulled ahead of its hold, so copies
/// fetched together do not all expire together (ADR#0049).
pub const RELEASE_CACHE_JITTER: Duration = Duration::from_secs(30);

/// The most releases one client holds at once (ADR#0023 Decision 6). A
/// release that finds the cache full is served without being held.
pub const MAX_CACHED_RELEASES: usize = 4_096;

/// How far back the invalidation reader starts reading: the longest any copy
/// of a release may be held, its hold bound plus the most jitter (ADR#0049).
pub const INVALIDATION_LOOKBACK: Duration = Duration::from_secs(330);

/// How long the invalidation reader waits before it reads again after it
/// lost its place.
pub const INVALIDATION_RETRY_DELAY: Duration = Duration::from_secs(1);

/// The stream release invalidations are published to.
pub const INVALIDATION_STREAM: &str = "SECRETS_INVALIDATION";

/// Every tenant's release invalidations:
/// `{tenant}.secrets.invalidation.v1.{owner_token}.{credential_id}`.
pub(crate) const INVALIDATION_FILTER: &str = "*.secrets.invalidation.v1.>";

/// The tokens between the tenant and the owner in an invalidation subject.
pub(crate) const INVALIDATION_DOMAIN: [&str; 3] = ["secrets", "invalidation", "v1"];
