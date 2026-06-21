use crate::denial_category::DenialCategory;

const MAX_LEN: usize = 256;

/// Wire-safe denial reason carried in `nats.error` (opaque category string only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenialReason(String);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DenialReasonError {
    #[error("denial reason must be non-empty")]
    Empty,
    #[error("denial reason must be at most {MAX_LEN} characters")]
    TooLong,
}

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

    #[test]
    fn display_renders_both_error_variants() {
        assert_eq!(DenialReasonError::Empty.to_string(), "denial reason must be non-empty");
        assert!(DenialReasonError::TooLong.to_string().contains("must be at most"));
    }
}
