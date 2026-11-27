use serde::{Deserialize, Serialize};

use super::constants::MAX_PROCESS_IDENTITY_BYTES;
use super::process_identity_error::ProcessIdentityError;

/// The pid and start time of a process, which together name one process instance.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) started: u64,
}

impl ProcessIdentity {
    /// Whether this names a real process: never pid 0 or 1, and a recorded start time.
    pub(crate) fn is_real(&self) -> bool {
        self.pid > 1 && self.started > 0
    }

    /// Parses the `<pid> <start ticks>` text a launcher script prints.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, ProcessIdentityError> {
        if bytes.len() > MAX_PROCESS_IDENTITY_BYTES {
            return Err(ProcessIdentityError::TooLong);
        }
        let text = std::str::from_utf8(bytes)?;
        let mut fields = text.split_whitespace();
        let pid = fields.next().ok_or(ProcessIdentityError::MissingPid)?.parse()?;
        let started = fields.next().ok_or(ProcessIdentityError::MissingStart)?.parse()?;
        let identity = Self { pid, started };
        if fields.next().is_some() || !identity.is_real() {
            return Err(ProcessIdentityError::Invalid);
        }
        Ok(identity)
    }
}
