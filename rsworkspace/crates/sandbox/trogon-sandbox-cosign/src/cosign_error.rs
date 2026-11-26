use trogon_sandbox_contract::Digest;

/// A signing or verification step failed.
#[derive(Debug, thiserror::Error)]
pub enum CosignError {
    #[error("cosign signature invalid for digest {digest}")]
    SignatureInvalid { digest: Digest },
    #[error("cosign signature for digest {digest} does not cover the expected annotations")]
    AnnotationMismatch { digest: Digest },
    #[error("cosign signing failed: {0}")]
    SignFailed(String),
    #[error("cosign verification failed: {0}")]
    VerifyFailed(String),
    #[error("cosign public key: {0}")]
    PublicKey(String),
    #[error("registry: {0}")]
    Registry(String),
}
