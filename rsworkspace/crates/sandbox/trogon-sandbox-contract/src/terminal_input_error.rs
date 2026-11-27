use crate::Base64DataError;

/// A terminal input write does not satisfy the rules [`TerminalInput`](crate::TerminalInput) enforces.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum TerminalInputError {
    #[error("terminal input is not base64")]
    InvalidBase64 {
        #[source]
        source: Base64DataError,
    },
    #[error("terminal input exceeds {max} bytes")]
    TooLarge { max: usize },
}
