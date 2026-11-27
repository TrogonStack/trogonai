/// Why a set of scan extraction limits was refused.
#[derive(Debug, thiserror::Error)]
pub enum ScanLimitsError {
    #[error("invalid extraction budget")]
    Invalid,
}
