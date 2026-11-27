use serde::Deserialize;

use crate::DaytonaExecError;
use crate::ordinary_completion::OrdinaryCompletion;
use crate::ordinary_identity::OrdinaryIdentity;
use crate::ordinary_phase::OrdinaryPhase;
use crate::ordinary_protocol::VERSION;
use crate::ordinary_root_exit::OrdinaryRootExit;

/// The state file the bridge keeps for one command.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrdinaryState {
    pub(crate) version: u32,
    pub(crate) execution_id: String,
    pub(crate) nonce: String,
    pub(crate) phase: OrdinaryPhase,
    pub(crate) helper: Option<OrdinaryIdentity>,
    pub(crate) root: Option<OrdinaryIdentity>,
    pub(crate) root_exit: Option<OrdinaryRootExit>,
    pub(crate) completion: Option<OrdinaryCompletion>,
}

impl OrdinaryState {
    /// Checks the state belongs to this execution and is internally consistent, and returns the supervisor and root
    /// identities it names, which stay fixed from the moment they are first seen.
    pub(crate) fn validate(
        &self,
        execution: &str,
        nonce: &str,
    ) -> Result<Option<(OrdinaryIdentity, OrdinaryIdentity)>, DaytonaExecError> {
        if self.version != VERSION || self.execution_id != execution || self.nonce != nonce {
            return Err(DaytonaExecError::OrdinaryUnconfirmed);
        }
        let identities = match (self.helper, self.root) {
            (Some(helper), Some(root))
                if helper.pid > 1
                    && root.pid > 1
                    && helper.pid != root.pid
                    && helper.start_time > 0
                    && root.start_time > 0 =>
            {
                Some((helper, root))
            }
            (None, None)
                if matches!(
                    self.phase,
                    OrdinaryPhase::Starting | OrdinaryPhase::Prepared | OrdinaryPhase::Failed
                ) =>
            {
                None
            }
            _ => return Err(DaytonaExecError::OrdinaryUnconfirmed),
        };
        if self.root_exit.is_some_and(|status| !status.is_valid()) {
            return Err(DaytonaExecError::OrdinaryUnconfirmed);
        }
        match (&self.phase, &self.completion) {
            (OrdinaryPhase::Absent, Some(completion))
                if completion.is_consistent() && self.root_exit == Some(completion.root_exit()) => {}
            (OrdinaryPhase::Absent, _) | (_, Some(_)) => return Err(DaytonaExecError::OrdinaryUnconfirmed),
            _ => {}
        }
        Ok(identities)
    }
}

#[cfg(test)]
mod tests;
