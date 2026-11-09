use spiffe::SpiffeId;

use crate::RegistrationError;
use crate::constants::MAX_SPIFFE_ID_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkloadId(SpiffeId);

impl WorkloadId {
    pub fn new(value: &str) -> Result<Self, RegistrationError> {
        if value.len() > MAX_SPIFFE_ID_BYTES {
            return Err(RegistrationError::Binding);
        }
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
