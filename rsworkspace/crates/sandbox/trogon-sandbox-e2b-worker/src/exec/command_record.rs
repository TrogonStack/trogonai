use crate::ulid::Ulid;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{ExecutionId, LaunchKey};

use super::command_owner::CommandOwner;
use super::command_phase::CommandPhase;
use super::command_record_error::CommandRecordError;
use super::constants::CUSTODY_RECORD_VERSION;
use super::execution::Execution;
use super::launch_fingerprint::LaunchFingerprint;
use crate::snapshots::CapturedExecution;

/// The durable state of one keyed command launch.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandRecord {
    pub(crate) version: u32,
    pub(crate) owner: CommandOwner,
    pub(crate) launch_key: LaunchKey,
    pub(crate) execution_id: Option<ExecutionId>,
    pub(crate) fingerprint: Option<LaunchFingerprint>,
    pub(crate) phase: CommandPhase,
    pub(crate) cancel_requested: bool,
    pub(crate) execution: Option<CapturedExecution>,
}

impl CommandRecord {
    pub(crate) fn reserved(owner: CommandOwner, launch_key: LaunchKey, fingerprint: LaunchFingerprint) -> Self {
        Self {
            version: CUSTODY_RECORD_VERSION,
            owner,
            launch_key,
            execution_id: Some(Execution::fresh_id()),
            fingerprint: Some(fingerprint),
            phase: CommandPhase::Reserved,
            cancel_requested: false,
            execution: None,
        }
    }

    pub(crate) fn canceled_before_start(owner: CommandOwner, launch_key: LaunchKey) -> Self {
        Self {
            version: CUSTODY_RECORD_VERSION,
            owner,
            launch_key,
            execution_id: None,
            fingerprint: None,
            phase: CommandPhase::Canceled,
            cancel_requested: true,
            execution: None,
        }
    }

    fn has_authority(&self) -> bool {
        let identified = self.execution_id.is_some() && self.fingerprint.is_some();
        match self.phase {
            CommandPhase::Reserved | CommandPhase::Preparing => identified && self.execution.is_none(),
            CommandPhase::Bound | CommandPhase::Running => identified && self.execution.is_some(),
            CommandPhase::Canceled => {
                self.cancel_requested
                    && (identified
                        || (self.execution_id.is_none() && self.fingerprint.is_none() && self.execution.is_none()))
            }
        }
    }

    pub(crate) fn validate(&self) -> Result<(), CommandRecordError> {
        if self.version != CUSTODY_RECORD_VERSION {
            return Err(CommandRecordError::Version);
        }
        self.owner.validate()?;
        if let Some(identity) = &self.execution_id {
            Ulid::parse(identity.as_str())?;
        }
        if !self.has_authority() {
            return Err(CommandRecordError::PhaseAuthority);
        }
        if let Some(execution) = &self.execution
            && (Some(execution.execution_id.to_string()) != self.execution_id.as_ref().map(ToString::to_string)
                || execution.identity.pid <= 1
                || execution.identity.started == 0
                || execution.deadline.is_none())
        {
            return Err(CommandRecordError::NativeBinding);
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<Bytes, CommandRecordError> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?.into())
    }
}
