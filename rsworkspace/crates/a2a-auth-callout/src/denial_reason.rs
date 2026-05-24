use std::fmt;

use crate::denial_category::DenialCategory;

const MAX_LEN: usize = 256;

/// Wire-safe denial reason carried in `nats.error` (opaque category string only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenialReason(String);

#[derive(Debug, PartialEq, Eq)]
pub enum DenialReasonError {
    Empty,
    TooLong,
}

impl fmt::Display for DenialReasonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("denial reason must be non-empty"),
            Self::TooLong => write!(f, "denial reason must be at most {MAX_LEN} characters"),
        }
    }
}

impl std::error::Error for DenialReasonError {}

impl DenialReason {
    pub fn new(category: DenialCategory) -> Result<Self, DenialReasonError> {
        Self::from_wire(category.as_str())
    }

    pub fn from_wire(reason: &str) -> Result<Self, DenialReasonError> {
        if reason.is_empty() {
            return Err(DenialReasonError::Empty);
        }
        if reason.len() > MAX_LEN {
            return Err(DenialReasonError::TooLong);
        }
        Ok(Self(reason.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        assert_eq!(DenialReason::from_wire("").unwrap_err(), DenialReasonError::Empty);
    }

    #[test]
    fn rejects_over_long() {
        let s = "a".repeat(MAX_LEN + 1);
        assert_eq!(DenialReason::from_wire(&s).unwrap_err(), DenialReasonError::TooLong);
    }

    #[test]
    fn accepts_category_string() {
        let r = DenialReason::new(DenialCategory::InvalidCredentials).unwrap();
        assert_eq!(r.as_str(), "invalid_credentials");
    }
}
