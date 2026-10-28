use super::header_text::{HeaderText, InvalidHeaderTextError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryReason(HeaderText);

impl RetryReason {
    pub fn new(reason: &str) -> Result<Self, InvalidHeaderTextError> {
        HeaderText::new(reason).map(Self)
    }

    pub fn socket(reason: &str) -> Result<Option<Self>, InvalidHeaderTextError> {
        if reason.is_empty() {
            return Ok(None);
        }
        Self::new(reason).map(Some)
    }

    pub fn as_header_text(&self) -> &HeaderText {
        &self.0
    }
}
