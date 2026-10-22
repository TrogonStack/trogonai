use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identifier(String);
impl Identifier {
    pub fn new(value: String) -> Result<Self, Error> {
        if value.is_empty() || value.len() > 63 || value.contains('\0') {
            return Err(Error::Identifier);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn quoted(&self) -> String {
        format!("\"{}\"", self.0.replace('"', "\"\""))
    }
}
