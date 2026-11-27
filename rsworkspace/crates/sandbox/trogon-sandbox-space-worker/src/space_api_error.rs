/// Why a SPACE client could not be built.
#[derive(Debug, thiserror::Error)]
#[error("SPACE HTTP client could not be built: {0}")]
pub struct SpaceApiError(#[from] pub reqwest::Error);
