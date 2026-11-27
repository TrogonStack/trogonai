use trogon_sandbox_contract::DigestError;
use trogon_sandbox_cosign::CosignError;
use trogon_sandbox_oci::OciError;

use super::{ExtractError, SyftError, TrivyError};

/// Why the post-export pipeline did not finish.
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("trivy gate: {0}")]
    Trivy(#[from] TrivyError),
    #[error("syft sbom: {0}")]
    Syft(#[from] SyftError),
    #[error("cosign: {0}")]
    Cosign(#[from] CosignError),
    #[error("oci: {0}")]
    Oci(#[from] OciError),
    #[error("scan input: {0}")]
    Extraction(#[from] ExtractError),
    #[error("scan input i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed {key} annotation: {source}")]
    MalformedEnvDigest {
        key: &'static str,
        #[source]
        source: DigestError,
    },
}
