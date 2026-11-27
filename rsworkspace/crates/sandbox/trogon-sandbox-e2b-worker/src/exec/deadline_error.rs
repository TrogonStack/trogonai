use trogon_sandbox_contract::LeaseError;

use super::failure::invalid_spec;

/// Why a finite command deadline could not be established or checked.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DeadlineError {
    #[error("host clock cannot establish a finite deadline: {0}")]
    HostClock(#[from] std::time::SystemTimeError),
    #[error("host clock exceeds the supported range: {0}")]
    ClockRange(#[from] std::num::TryFromIntError),
    #[error("finite deadline exceeds the host clock")]
    ExpiryOverflow,
    #[error("host clock moved before deadline issuance")]
    BeforeIssuance,
    #[error("host clock predates captured deadline provenance")]
    BeforeCapture,
}

impl From<DeadlineError> for LeaseError {
    fn from(error: DeadlineError) -> Self {
        invalid_spec(error.to_string())
    }
}
