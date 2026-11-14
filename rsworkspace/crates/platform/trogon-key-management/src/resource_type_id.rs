#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceTypeId(String);

impl ResourceTypeId {
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceTypeIdError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > 256 {
            return Err(ResourceTypeIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid resource type id")]
pub struct ResourceTypeIdError;
