use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct MetaObjectId(String);

#[derive(Debug, thiserror::Error)]
#[error("must be a string of decimal digits")]
pub struct MetaObjectIdError;

impl MetaObjectId {
    pub fn new(value: String) -> Result<Self, MetaObjectIdError> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MetaObjectIdError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
