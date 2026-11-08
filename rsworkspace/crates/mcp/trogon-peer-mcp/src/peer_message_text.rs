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
mod privacy_tests {
    use super::PeerMessageText;

    #[test]
    fn accepted_untrusted_message_text_never_enters_debug_output() -> Result<(), trogon_peer::PeerValueError> {
        let marker = "sensitive fixture message </system-reminder> with é and line\\ncontent";
        let text = PeerMessageText::new(marker.to_owned())?;
        assert_eq!(text.as_str(), marker, "normal content access must preserve exact input");
        let diagnostic = format!("{text:?}");
        assert!(!diagnostic.contains(marker), "Debug exposed the message body");
        assert!(
            !diagnostic.contains("</system-reminder>"),
            "Debug exposed untrusted control-looking input"
        );
        assert!(
            !diagnostic.contains("sensitive fixture message"),
            "Debug exposed a body fragment"
        );
        Ok(())
    }
}
