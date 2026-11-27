use trogon_sandbox_protocol::ReadFileError;

use crate::{ProxmoxApiError, ProxmoxProviderError};

/// The end-to-end check of a live Proxmox host failed, or its VM could not be cleaned up.
#[derive(Debug, thiserror::Error)]
pub enum SpikeError {
    #[error(transparent)]
    Provider(#[from] ProxmoxProviderError),
    #[error(transparent)]
    ReadFile(#[from] ReadFileError<ProxmoxProviderError>),
    #[error(transparent)]
    Api(#[from] ProxmoxApiError),
    #[error("the check's VM could not be cleaned up: {0}")]
    Cleanup(ProxmoxProviderError),
    #[error("{0}")]
    Mismatch(&'static str),
    #[error("the check did not finish within {seconds} seconds")]
    TimedOut { seconds: u64 },
    #[error("the check's request or report could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("the check's report could not be written: {0}")]
    Report(#[from] std::io::Error),
}
