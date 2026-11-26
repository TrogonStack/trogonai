use std::time::Duration;

/// JSON field the broker adds to a signed dispatch.
pub const AUTH_FIELD: &str = "dispatch_auth";
/// Operation name of a lease launch dispatch.
pub const OP_LEASE_DISPATCH: &str = "lease_dispatch";
/// Header the broker adds to a signed per-lease request.
pub const AUTH_HEADER: &str = "Sandbox-Dispatch-Auth";

/// Environment variable naming the file that holds the broker's base64 signing seed.
pub const ENV_SIGNING_KEY_FILE: &str = "SANDBOX_DISPATCH_SIGNING_KEY_FILE";
/// Environment variable holding the broker's base64 signing seed.
pub const ENV_SIGNING_KEY: &str = "SANDBOX_DISPATCH_SIGNING_KEY";
/// Environment variable naming the signing key.
pub const ENV_SIGNING_KEY_ID: &str = "SANDBOX_DISPATCH_SIGNING_KEY_ID";
/// Environment variable holding the signed envelope lifetime in seconds.
pub const ENV_SIGNING_TTL_SECS: &str = "SANDBOX_DISPATCH_TTL_SECS";
/// Environment variable naming the file that holds the workers' public keys.
pub const ENV_PUBLIC_KEYS_FILE: &str = "SANDBOX_DISPATCH_PUBLIC_KEYS_FILE";
/// Environment variable holding the workers' public keys.
pub const ENV_PUBLIC_KEYS: &str = "SANDBOX_DISPATCH_PUBLIC_KEYS";
/// Environment variable holding the verification [`EnforcementMode`](crate::EnforcementMode).
pub const ENV_VERIFY_MODE: &str = "SANDBOX_DISPATCH_VERIFY";
/// How the worker treats a request whose lease epoch is older than the held one. Defaults to `warn` when unset.
pub const ENV_FENCE_EPOCH: &str = "SANDBOX_FENCE_EPOCH";
/// File that keeps the lease-to-tenant fence across a worker restart.
pub const ENV_FENCE_FILE: &str = "SANDBOX_DISPATCH_FENCE_FILE";

pub(crate) const FENCE_FILE_NAME: &str = "dispatch-fence.jsonl";
pub(crate) const DEFAULT_TTL: Duration = Duration::from_secs(300);
pub(crate) const CLOCK_SKEW: Duration = Duration::from_secs(30);
pub(crate) const SIGNING_DOMAIN: &str = "sandbox-dispatch-auth-v1";
/// Header `nats_transfer` sets when the body is a descriptor of a larger one.
pub(crate) const TRANSFER_HEADER: &str = "Sandbox-Transfer";
