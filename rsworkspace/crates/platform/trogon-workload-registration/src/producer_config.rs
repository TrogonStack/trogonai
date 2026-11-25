use std::num::NonZeroU64;

use ed25519_dalek::VerifyingKey;

use crate::{ApprovalKey, RegistrationError, RegistrationScope, ScopeName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerConfig {
    scope: RegistrationScope,
    recovery_fence: NonZeroU64,
    approval_key: ApprovalKey,
}

impl ProducerConfig {
    pub fn new(
        tenant: &str,
        project: &str,
        trust_domain: &str,
        recovery_fence: NonZeroU64,
        approval_key: VerifyingKey,
    ) -> Result<Self, RegistrationError> {
        Ok(Self {
            scope: RegistrationScope::new(ScopeName::new(tenant)?, ScopeName::new(project)?, trust_domain)?,
            recovery_fence,
            approval_key: ApprovalKey::new(approval_key)?,
        })
    }

    pub fn scope(&self) -> &RegistrationScope {
        &self.scope
    }
    pub fn recovery_fence(&self) -> NonZeroU64 {
        self.recovery_fence
    }
    pub fn approval_key(&self) -> &ApprovalKey {
        &self.approval_key
    }
}
