/// Why a recovered native access token cannot be used.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("native access token is missing or invalid")]
pub struct NativeAccessTokenError;
