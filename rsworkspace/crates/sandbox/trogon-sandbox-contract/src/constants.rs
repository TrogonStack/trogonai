pub(crate) const MAXIMUM_BYTES: usize = 128;
pub(crate) const DIGEST_PREFIX: &str = "sha256:";
pub(crate) const DIGEST_HASH_BYTES: usize = 64;
/// Longest Unix user or group name the wire accepts, in bytes.
pub(crate) const MAX_UNIX_NAME_LEN: usize = 32;
/// The environment variable a worker sets to a command's execution id.
pub(crate) const EXECUTION_ID_ENV: &str = "SANDBOX_EXECUTION_ID";
/// How long a command may run when its request names no timeout, in seconds.
pub(crate) const DEFAULT_COMMAND_TIMEOUT_SECONDS: u32 = 60;
