#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedContentRef(String);
impl SealedContentRef {
    pub fn new(value: impl Into<String>) -> Result<Self, super::PeerValueError> {
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
