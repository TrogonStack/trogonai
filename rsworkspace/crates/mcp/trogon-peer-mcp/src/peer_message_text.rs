use std::fmt;

use trogon_peer::PeerValueError;
use trogon_peer_context::MAX_MESSAGE_BYTES;

pub(crate) struct PeerMessageText(String);

impl PeerMessageText {
    pub(crate) fn new(text: String) -> Result<Self, PeerValueError> {
        if !(1..=MAX_MESSAGE_BYTES).contains(&text.len()) {
            return Err(PeerValueError::InvalidContent);
        }
        Ok(Self(text))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PeerMessageText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerMessageText")
            .field("utf8_bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
