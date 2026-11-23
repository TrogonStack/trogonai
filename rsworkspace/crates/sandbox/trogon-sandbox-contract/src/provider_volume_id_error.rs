/// A provider volume identifier is empty, too long, or contains whitespace or a control character.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ProviderVolumeIdError {
    #[error("invalid provider volume ID")]
    Invalid,
}
