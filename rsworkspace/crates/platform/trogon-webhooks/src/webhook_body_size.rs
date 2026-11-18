use crate::constants::MAX_BODY_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebhookBodySize(usize);

#[derive(Debug, thiserror::Error)]
#[error("webhook body limit must be between 1 and 1048576 bytes")]
pub struct WebhookBodySizeError;

impl WebhookBodySize {
    pub fn new(value: usize) -> Result<Self, WebhookBodySizeError> {
        if value == 0 || value > MAX_BODY_BYTES {
            return Err(WebhookBodySizeError);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> usize {
        self.0
    }
}
