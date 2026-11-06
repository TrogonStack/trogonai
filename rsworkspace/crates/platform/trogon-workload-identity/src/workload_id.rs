use spiffe::SpiffeId;

use crate::RegistrationError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkloadId(SpiffeId);

impl WorkloadId {
    pub fn new(value: &str) -> Result<Self, RegistrationError> {
        let id = SpiffeId::new(value).map_err(|_| RegistrationError::Binding)?;
        if id.to_string() != value || id.path().is_empty() {
            return Err(RegistrationError::Binding);
        }
        Ok(Self(id))
    }

    pub fn as_spiffe_id(&self) -> &SpiffeId {
        &self.0
    }
}
