use std::num::NonZeroU32;
use std::time::Duration;

/// The queue group that spreads lease dispatches across the workers of this provider kind.
pub const LEASE_QUEUE_GROUP: &str = "sandbox-provider-azure_container_apps";

/// The Dynamic Sessions data plane API version the client speaks unless told otherwise.
pub const DEFAULT_API_VERSION: &str = "2024-02-02-preview";

/// The login authority a service principal token is requested from unless told otherwise.
pub const DEFAULT_LOGIN_ENDPOINT: &str = "https://login.microsoftonline.com";

/// The OAuth scope that grants access to the Dynamic Sessions data plane.
pub const DYNAMIC_SESSIONS_SCOPE: &str = "https://dynamicsessions.io/.default";

/// The longest a lease may live, and the credential lifetime a lease without a TTL gets.
pub const MAX_LEASE_LIFETIME_SECONDS: NonZeroU32 = NonZeroU32::MIN.saturating_add(86_399);

/// How long the credential that deletes a session stays valid.
pub(crate) const CLEANUP_CREDENTIAL_TTL_SECONDS: NonZeroU32 = NonZeroU32::MIN.saturating_add(299);

/// How long the credential that reads or writes a volume stays valid.
pub(crate) const VOLUME_CREDENTIAL_TTL_SECONDS: NonZeroU32 = NonZeroU32::MIN.saturating_add(3_599);

/// How many times a fresh session is polled before it is declared not ready.
pub(crate) const SESSION_READY_ATTEMPTS: u32 = 60;

/// The pause between two polls of a fresh session.
pub(crate) const SESSION_READY_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How often allocations whose cleanup failed are retried.
pub const CLEANUP_RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Why a snapshot image is refused.
pub(crate) const SNAPSHOT_UNSUPPORTED_REASON: &str = "ACA Dynamic Sessions has no native snapshot export or restore; use \
     SandboxImage::Registry with a pool container image.";
