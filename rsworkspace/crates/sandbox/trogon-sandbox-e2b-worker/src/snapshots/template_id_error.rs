/// Why a string is not a canonical E2B snapshot template id.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotTemplateIdError {
    #[error("invalid canonical E2B snapshot template id")]
    Invalid,
}
