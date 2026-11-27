/// The text is not a fence mode.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("unknown fence mode, expected 'optional' or 'required'")]
pub struct FenceModeError;
