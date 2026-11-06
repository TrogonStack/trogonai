use std::num::NonZeroU64;

use ed25519_dalek::VerifyingKey;

use crate::RegistrationError;

#[derive(Debug)]
pub struct ProducerConfig;

impl ProducerConfig {
    pub fn new(
        _tenant: &str,
        _project: &str,
        _trust_domain: &str,
        _recovery_fence: NonZeroU64,
        _approval_key: VerifyingKey,
    ) -> Result<Self, RegistrationError> {
        Ok(Self)
    }
}
