use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectId(String);
impl ObjectId {
    pub fn new(value: String) -> Result<Self, Error> {
        if value.is_empty()
            || value.len() > crate::constants::MAX_OBJECT_ID_BYTES
            || !value.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-/".contains(&c))
        {
            return Err(Error::ObjectId);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
