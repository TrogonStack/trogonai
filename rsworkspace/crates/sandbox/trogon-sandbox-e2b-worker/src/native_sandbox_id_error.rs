/// Why a native sandbox identity was refused.
#[derive(Debug, thiserror::Error)]
#[error("invalid canonical E2B sandbox identity")]
pub struct NativeSandboxIdError;
