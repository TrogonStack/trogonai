/// Why an agent image reference is not acceptable.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ImageDigestError {
    #[error("container image reference exceeds the v1 limit")]
    TooLong,
    #[error("container image must be a valid OCI reference")]
    InvalidReference,
    #[error("container image must be pinned by digest")]
    NotPinned,
    #[error("only sha256 image digests are supported")]
    UnsupportedAlgorithm,
    #[error("container image requires a complete lowercase sha256 digest")]
    MalformedDigest,
}
