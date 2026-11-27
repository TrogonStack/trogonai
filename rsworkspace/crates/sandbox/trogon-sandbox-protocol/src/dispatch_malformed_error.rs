/// Why a signed dispatch envelope or request could not be understood.
#[derive(Debug, thiserror::Error)]
pub enum DispatchMalformedError {
    #[error("not an object")]
    NotAnObject,
    #[error("version {version}")]
    UnsupportedVersion { version: u8 },
    #[error("signature")]
    Signature,
    #[error("nonce")]
    Nonce,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("command readiness cannot carry a transfer descriptor")]
    ReadinessTransfer,
    #[error("native snapshot deletion cannot carry a transfer descriptor")]
    NativeSnapshotDeleteTransfer,
    #[error("owned cancellation cannot carry a transfer descriptor")]
    OwnedCancellationTransfer,
    #[error("native snapshot provider differs from its subject")]
    NativeSnapshotProvider,
    #[error("captured scope is not worker command readiness")]
    CapturedScopeNotReadiness,
}
