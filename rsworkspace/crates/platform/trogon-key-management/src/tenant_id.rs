#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TenantId(String);

impl TenantId {
    pub fn new(value: impl Into<String>) -> Result<Self, TenantIdError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > 256 {
            return Err(TenantIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid tenant id")]
pub struct TenantIdError;
