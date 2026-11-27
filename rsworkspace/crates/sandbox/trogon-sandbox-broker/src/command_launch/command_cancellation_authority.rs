use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandCancellationAuthority {
    version: u32,
    tenant: TenantId,
    identity: LeaseAttemptIdentity,
    sandbox_id: String,
    launch_key: LaunchKey,
    pub(super) fingerprint: String,
}

impl CommandCancellationAuthority {
    pub fn for_launch(record: &LeaseRecord, key: &LaunchKey, fingerprint: &str) -> Option<Self> {
        let worker = record.worker_id.as_ref()?;
        let attempt = record.dispatch_attempt.as_ref()?;
        let epoch = record.epoch?;
        if attempt.epoch != Some(epoch) {
            return None;
        }
        let authority = Self {
            version: 1,
            tenant: record.tenant.clone(),
            identity: LeaseAttemptIdentity {
                lease_id: record.lease_id.clone(),
                provider: record.provider,
                target: DispatchTarget {
                    worker_id: worker.clone(),
                    attempt_id: attempt.id.clone(),
                    epoch: Some(epoch),
                },
            },
            sandbox_id: record.sandbox_id.clone(),
            launch_key: key.clone(),
            fingerprint: fingerprint.into(),
        };
        authority.validate(&record.lease_id, key).ok()?;
        Some(authority)
    }

    pub(super) fn validate(&self, lease: &LeaseId, key: &LaunchKey) -> Result<(), CommandLaunchError> {
        if !(self.version == 1
            && self.identity.lease_id == *lease
            && self.launch_key == *key
            && [
                self.tenant.as_str(),
                self.identity.lease_id.as_str(),
                self.sandbox_id.as_str(),
            ]
            .iter()
            .all(|value| !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control))
            && self.identity.target.epoch.is_some()
            && self.fingerprint.len() == 64
            && self.fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            && serde_json::to_vec(self)?.len() <= MAX_CANCELLATION_AUTHORITY_BYTES)
        {
            return Err(CommandLaunchError::InvalidAuthority);
        }
        Ok(())
    }

    pub fn matches_record(&self, record: &LeaseRecord) -> bool {
        Self::for_launch(record, &self.launch_key, &self.fingerprint).as_ref() == Some(self)
    }

    pub fn scope(&self) -> VerbScope {
        VerbScope {
            tenant: self.tenant.clone(),
            lease_id: self.identity.lease_id.clone(),
            worker_id: Some(self.identity.target.worker_id.clone()),
            attempt_id: Some(self.identity.target.attempt_id.clone()),
            epoch: self.identity.target.epoch,
        }
    }
}
