use serde::Deserialize;

/// What the QEMU guest agent reports about a command it ran.
#[derive(Debug, Deserialize)]
pub(crate) struct GuestProcess {
    #[serde(default)]
    pub(crate) exited: Option<u8>,
    pub(crate) exitcode: Option<i32>,
    #[serde(default, rename = "out-data")]
    pub(crate) stdout: String,
    #[serde(default, rename = "out-truncated")]
    pub(crate) stdout_truncated: Option<u8>,
    #[serde(default, rename = "err-truncated")]
    pub(crate) stderr_truncated: Option<u8>,
}
