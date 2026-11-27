use serde::Deserialize;

use crate::ordinary_root_exit::OrdinaryRootExit;
use crate::sha256_pin::Sha256Pin;

/// What the bridge recorded when the command's whole process family was gone.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrdinaryCompletion {
    pub(crate) released: bool,
    pub(crate) root_exit_code: Option<i32>,
    pub(crate) root_signal: Option<i32>,
    pub(crate) timed_out: bool,
    pub(crate) canceled: bool,
    pub(crate) reaped_count: u64,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) stdout_len: u64,
    pub(crate) stderr_len: u64,
    pub(crate) stdout_sha256: String,
    pub(crate) stderr_sha256: String,
}

impl OrdinaryCompletion {
    pub(crate) fn root_exit(&self) -> OrdinaryRootExit {
        OrdinaryRootExit {
            exit_code: self.root_exit_code,
            signal: self.root_signal,
        }
    }

    /// Whether the record is internally consistent: released, reaped, a valid exit, a deadline only by cancelation
    /// and well-formed stream digests.
    pub(crate) fn is_consistent(&self) -> bool {
        self.released
            && self.reaped_count != 0
            && self.root_exit().is_valid()
            && (!self.timed_out || self.canceled)
            && Sha256Pin::parse(&self.stdout_sha256).is_ok()
            && Sha256Pin::parse(&self.stderr_sha256).is_ok()
    }
}
