/// A number is not a mandatory deny search depth.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("the mandatory deny search depth must be from {min} to {max}, not {got}")]
pub struct MandatoryDenySearchDepthError {
    pub min: u8,
    pub max: u8,
    pub got: u8,
}
