use trogon_sandbox_contract::DigestError;

/// Why a bind that carried an entropy seed could not be answered with a receipt.
#[derive(Debug, thiserror::Error)]
pub enum EntropyReseedError {
    #[error("kernel entropy reseed failed: {0}")]
    Kernel(#[from] std::io::Error),
    #[error("seed digest is not a valid digest: {0}")]
    Digest(#[from] DigestError),
}
