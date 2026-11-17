use crate::RegistrationError;
use crate::scope_name::ScopeName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkloadTenant(ScopeName);

impl WorkloadTenant {
    pub fn new(value: &str) -> Result<Self, RegistrationError> {
        ScopeName::new(value).map(Self)
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
