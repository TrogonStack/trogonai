use std::num::NonZeroU64;

use crate::RegistrationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityVersion {
    fence: NonZeroU64,
    registration: NonZeroU64,
    trust: NonZeroU64,
}

impl AuthorityVersion {
    pub fn new(fence: NonZeroU64, registration: NonZeroU64, trust: NonZeroU64) -> Self {
        Self {
            fence,
            registration,
            trust,
        }
    }
    pub fn recovery_fence(&self) -> NonZeroU64 {
        self.fence
    }
    pub fn registration_revision(&self) -> NonZeroU64 {
        self.registration
    }
    pub fn trust_revision(&self) -> NonZeroU64 {
        self.trust
    }

    pub(crate) fn check_successor(&self, previous: Self) -> Result<(), RegistrationError> {
        if self.fence != previous.fence || self.registration < previous.registration || self.trust < previous.trust {
            return Err(RegistrationError::Revision);
        }
        Ok(())
    }
}
