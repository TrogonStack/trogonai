use std::time::Duration;

/// Length of the nonce that accompanies every ciphertext, in bytes.
pub const NONCE_LEN: usize = 12;

pub(crate) const KEY_LEN: usize = 32;

pub(crate) const RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 365);

pub(crate) const ISSUE_PREFIX: &str = "sandbox.credentials.issue.";
pub(crate) const REVOKE_SUBJECT: &str = "sandbox.credentials.revoke.>";
pub(crate) const ROTATE_PREFIX: &str = "sandbox.credentials.rotate.";
pub(crate) const ISSUE_SUBJECT: &str = "sandbox.credentials.issue.>";
pub(crate) const ROTATE_SUBJECT: &str = "sandbox.credentials.rotate.>";

pub(crate) const PULL_SECRET_PREFIX: &str = "pull_secret.";

/// Record format 1: AES-256-GCM under the process key, no associated data.
pub(crate) const FORMAT_LEGACY: u32 = 1;
/// Record format 2: per-record data key wrapped under the root key with the tenant as AAD; the payload AAD binds
/// provider, tenant, credential id and version.
pub(crate) const FORMAT_ENVELOPE: u32 = 2;

/// How many times a commit retries after losing a race for the same lease.
pub(crate) const DEFAULT_COMMIT_ATTEMPTS: usize = 8;
