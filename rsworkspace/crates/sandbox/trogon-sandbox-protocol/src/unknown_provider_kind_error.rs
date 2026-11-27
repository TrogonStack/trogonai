/// A provider name matches no known provider or alias.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("unknown provider kind: {name}")]
pub struct UnknownProviderKindError {
    name: String,
}

impl UnknownProviderKindError {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    /// The name that matched no provider.
    pub fn name(&self) -> &str {
        &self.name
    }
}
