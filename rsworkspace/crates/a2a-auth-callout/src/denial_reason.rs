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
mod tests;
