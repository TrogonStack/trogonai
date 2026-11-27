/// Why a provider dispatch payload could not be built.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct DispatchPayloadError(#[from] serde_json::Error);
