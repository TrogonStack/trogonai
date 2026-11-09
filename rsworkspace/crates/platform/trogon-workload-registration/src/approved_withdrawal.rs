use crate::{ApprovedRegistration, ProducerConfig, RegistrationError};

#[derive(Debug)]
pub(crate) struct ApprovedWithdrawal(ApprovedRegistration);

impl ApprovedWithdrawal {
    pub(crate) fn new(config: &ProducerConfig, registration: ApprovedRegistration) -> Result<Self, RegistrationError> {
        registration.check_current()?;
        if registration.config() != config {
            return Err(RegistrationError::Scope);
        }
        if !registration.is_withdrawal() {
            return Err(RegistrationError::NotWithdrawal);
        }
        Ok(Self(registration))
    }

    pub(crate) fn registration(&self) -> &ApprovedRegistration {
        &self.0
    }
    pub(crate) fn into_registration(self) -> ApprovedRegistration {
        self.0
    }
}
