/// The sandbox class name was not one this worker serves.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("unknown substrate sandbox class {class:?}, expected 'gvisor' or 'microvm'")]
pub struct SandboxClassError {
    class: String,
}

impl SandboxClassError {
    pub(super) fn new(class: &str) -> Self {
        Self {
            class: class.to_string(),
        }
    }

    /// The name that was rejected, lowercased.
    pub fn class(&self) -> &str {
        &self.class
    }
}
