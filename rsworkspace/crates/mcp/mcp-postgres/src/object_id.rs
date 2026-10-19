use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectId(String);
impl ObjectId {
    pub fn new(value: String) -> Result<Self, Error> {
        if value.is_empty()
            || value.len() > 200
            || !value.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-./".contains(&c))
        {
            return Err(Error::ObjectId);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
