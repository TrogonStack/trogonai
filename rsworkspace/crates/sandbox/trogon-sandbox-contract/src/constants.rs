pub(crate) const MAXIMUM_BYTES: usize = 128;
pub(crate) const DIGEST_PREFIX: &str = "sha256:";
pub(crate) const DIGEST_HASH_BYTES: usize = 64;
/// Longest Unix user or group name the wire accepts, in bytes.
pub(crate) const MAX_UNIX_NAME_LEN: usize = 32;
/// The environment variable a worker sets to a command's execution id.
pub(crate) const EXECUTION_ID_ENV: &str = "SANDBOX_EXECUTION_ID";
/// How long a command may run when its request names no timeout, in seconds.
pub(crate) const DEFAULT_COMMAND_TIMEOUT_SECONDS: u32 = 60;
/// UTF-16 code units the glob dialect treats specially.
pub(crate) const GLOB_DASH: u16 = b'-' as u16;
pub(crate) const GLOB_STAR: u16 = b'*' as u16;
pub(crate) const GLOB_OPEN: u16 = b'[' as u16;
pub(crate) const GLOB_CLOSE: u16 = b']' as u16;
pub(crate) const GLOB_QUESTION: u16 = b'?' as u16;
