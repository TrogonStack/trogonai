use std::num::NonZeroU32;
use std::time::Duration;

/// Transports a clone may use. `ext::` and its kin are absent on purpose.
pub(crate) const ALLOWED_CLONE_SCHEMES: [&str; 5] = ["file", "git", "http", "https", "ssh"];

/// The user name that carries a bearer token in a clone url.
pub(crate) const BEARER_USERNAME: &str = "oauth2";

pub(crate) const BRIDGE_SOURCE: &str = include_str!("modal_bridge.py");

pub(crate) const BRIDGE_SCRIPT_NAME: &str = "modal_bridge.py";

pub(crate) const COLD_START_P50_MS: u32 = 5000;

/// How long a volume credential stays valid once issued.
pub(crate) const CREDENTIAL_TTL_SECONDS: NonZeroU32 = NonZeroU32::MIN.saturating_add(3599);

pub(crate) const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);

/// The most cores a sandbox may ask for unless the operator raises it.
pub const DEFAULT_MAX_VCPUS: u32 = 8;

pub(crate) const IMAGE_DIGEST_ENV: &str = "TROGON_SANDBOX_MODAL_IMAGE_DIGEST";

pub(crate) const IMAGE_ENV: &str = "TROGON_SANDBOX_MODAL_IMAGE";

pub(crate) const IMAGE_TAG_ENV: &str = "TROGON_SANDBOX_MODAL_IMAGE_TAG";

/// Every lease dispatch for Modal goes to one member of this queue group.
pub(crate) const LEASE_QUEUE_GROUP: &str = "sandbox-provider-modal";

pub(crate) const LEASE_TAG: &str = "trogon.lease_id";

pub(crate) const MAX_LEASE_LIFETIME_SECONDS: u32 = 86_400;

pub(crate) const MAX_SHA_LEN: usize = 64;

/// Modal documents a floor of 0.125 cores, advertised in the wire's thousandths.
pub(crate) const MIN_VCPU_MILLIS: u32 = 125;

pub(crate) const MIN_SHA_LEN: usize = 4;

pub(crate) const NAME_TAG: &str = "trogon.name";

/// How long Modal keeps a filesystem snapshot image. Modal's own default is 30 days; naming it here lets the
/// receipt carry the expiry instead of assuming it.
pub(crate) const NATIVE_SNAPSHOT_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

/// Remote path the workspace is staged at inside the Modal sandbox.
pub const SANDBOX_WORKDIR: &str = "/mnt/workspace";

/// How long the bridge waits on a filesystem snapshot before Modal cancels it.
pub(crate) const SNAPSHOT_TIMEOUT_SECONDS: u64 = 300;

pub(crate) const SNAPSHOT_UNSUPPORTED_REASON: &str = "this worker does not mirror Modal-side image or snapshot \
     artifacts into the Trogon snapshot store; use SandboxImage::Registry with an image Modal can pull.";

pub(crate) const TENANT_TAG: &str = "trogon.tenant";

pub(crate) const WAIT_ATTEMPTS: u32 = 120;

pub(crate) const WAIT_INTERVAL: Duration = Duration::from_millis(500);
