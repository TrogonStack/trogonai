/// Why a SPACE template name was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SpaceTemplateError {
    /// The name is empty.
    #[error("SPACE template is empty")]
    Empty,
    /// The name holds a space or a NUL byte.
    #[error("SPACE template holds a space or NUL byte")]
    Character,
}
