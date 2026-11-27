use serde::{Deserialize, Serialize};

use super::captured_deadline_error::CapturedDeadlineError;

/// The deadline state a command had when its snapshot was captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CapturedDeadline {
    Lease,
    Timeout {
        issued_unix_ms: u64,
        expires_unix_ms: u64,
        captured_unix_ms: u64,
        remaining_ms: u64,
        timed_out: bool,
    },
    Terminal {
        exit_code: i32,
    },
}

impl CapturedDeadline {
    pub(super) fn validate(&self) -> Result<(), CapturedDeadlineError> {
        if let Self::Timeout {
            issued_unix_ms,
            expires_unix_ms,
            captured_unix_ms,
            remaining_ms,
            ..
        } = self
        {
            let consistent = *issued_unix_ms > 0
                && expires_unix_ms > issued_unix_ms
                && captured_unix_ms >= issued_unix_ms
                && *remaining_ms <= expires_unix_ms.saturating_sub(*captured_unix_ms);
            if !consistent {
                return Err(CapturedDeadlineError::InvalidProvenance);
            }
        }
        Ok(())
    }
}
