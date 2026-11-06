use crate::{ProducerConfig, RegistrationError};

#[derive(Debug)]
pub struct ApprovedRegistration;

impl ApprovedRegistration {
    pub fn verify(
        _config: &ProducerConfig,
        _payload: &[u8],
        _signature: &[u8; 64],
    ) -> Result<Self, RegistrationError> {
        Ok(Self)
    }
}
