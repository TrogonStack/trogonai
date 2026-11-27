use bytes::Bytes;
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{LeaseAttemptIdentity, ProviderKind, TenantId};

use super::assignments_error::AssignmentsError;
use super::constants::{MAX_RECORD_BYTES, RECORD_VERSION};
use super::recovery::Recovery;
use super::sandbox_id::validate_sandbox_id;

/// The tenant, attempt and native sandbox one targeted lease is bound to.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Assignment {
    version: u32,
    pub tenant: TenantId,
    pub identity: LeaseAttemptIdentity,
    pub sandbox_id: Option<String>,
    pub recovery: Option<Recovery>,
}

impl Assignment {
    pub(super) fn reserved(tenant: TenantId, identity: LeaseAttemptIdentity, recovery: Option<Recovery>) -> Self {
        Self {
            version: RECORD_VERSION,
            tenant,
            identity,
            sandbox_id: None,
            recovery,
        }
    }

    pub(super) fn validate(&self) -> Result<(), AssignmentsError> {
        if self.version != RECORD_VERSION {
            return Err(AssignmentsError::UnsupportedVersion);
        }
        if self.identity.provider != ProviderKind::E2b || self.identity.target.epoch.is_none() {
            return Err(AssignmentsError::NotExact);
        }
        if let Some(id) = &self.sandbox_id {
            validate_sandbox_id(id)?;
            if self.recovery.is_none() {
                return Err(AssignmentsError::MissingRecovery);
            }
        }
        if let Some(recovery) = &self.recovery {
            recovery.validate()?;
        }
        Ok(())
    }

    pub(super) fn bytes(&self) -> Result<Bytes, AssignmentsError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(AssignmentsError::TooLarge);
        }
        Ok(bytes.into())
    }
}
