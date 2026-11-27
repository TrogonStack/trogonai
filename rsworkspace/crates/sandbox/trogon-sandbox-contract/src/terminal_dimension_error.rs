/// A terminal dimension is outside the accepted range.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum TerminalDimensionError {
    #[error("terminal dimension must be between 1 and {max}")]
    OutOfRange { max: u16 },
}
