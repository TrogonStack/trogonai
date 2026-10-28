use sha2::{Digest, Sha256};

use super::header_text::HeaderText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnroutableReason {
    InvalidJson,
    UnhandledPayloadType,
    MissingEventType,
    MissingEventId,
    InvalidUtf8Form,
    UnrecognizedForm,
    InvalidInteractionJson,
    MissingInteractionType,
    MissingCommand,
    MissingCommandTriggerId,
    InvalidHeaderValue,
    InvalidSubjectToken,
    UnhandledSocketModeType,
    MissingEnvelopeId,
    MissingSocketModePayload,
}

impl UnroutableReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::UnhandledPayloadType => "unhandled_payload_type",
            Self::MissingEventType => "missing_event_type",
            Self::MissingEventId => "missing_event_id",
            Self::InvalidUtf8Form => "invalid_utf8_form",
            Self::UnrecognizedForm => "unrecognized_form",
            Self::InvalidInteractionJson => "invalid_interaction_json",
            Self::MissingInteractionType => "missing_interaction_type",
            Self::MissingCommand => "missing_command",
            Self::MissingCommandTriggerId => "missing_command_trigger_id",
            Self::InvalidHeaderValue => "invalid_header_value",
            Self::InvalidSubjectToken => "invalid_subject_token",
            Self::UnhandledSocketModeType => "unhandled_socket_mode_type",
            Self::MissingEnvelopeId => "missing_envelope_id",
            Self::MissingSocketModePayload => "missing_socket_mode_payload",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnroutableMessageId(HeaderText);

impl UnroutableMessageId {
    pub fn new(reason: UnroutableReason, body: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(reason.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(body);
        Self(HeaderText::digest("unroutable-", &hasher.finalize()))
    }

    pub fn header(&self) -> &HeaderText {
        &self.0
    }
}

#[cfg(test)]
mod tests;
