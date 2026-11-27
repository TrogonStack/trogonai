/// The image reference a provider boots, as the engine spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef(pub String);

impl ImageRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
