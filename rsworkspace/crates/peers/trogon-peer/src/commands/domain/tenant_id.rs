use super::PeerValueError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantId(String);

impl TenantId {
    pub fn new(value: impl Into<String>) -> Result<Self, PeerValueError> {
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
