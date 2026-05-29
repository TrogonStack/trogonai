use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TopNError {
    InvalidN,
}

impl fmt::Display for TopNError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidN => f.write_str("n must be greater than zero"),
        }
    }
}

impl std::error::Error for TopNError {}

pub(crate) fn validate_n(n: usize) -> Result<(), TopNError> {
    if n == 0 {
        Err(TopNError::InvalidN)
    } else {
        Ok(())
    }
}
