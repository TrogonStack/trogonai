use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use serde::Serialize;
use trogon_sandbox_client::EvidenceFrame;

/// An evidence frame as it travels on the wire. Binary payloads are base64 behind a `base64:` prefix.
#[derive(Debug, Serialize)]
pub(crate) struct EvidenceEventBody {
    subject_suffix: String,
    payload: String,
}

impl From<EvidenceFrame> for EvidenceEventBody {
    fn from(frame: EvidenceFrame) -> Self {
        Self {
            payload: payload_text(&frame.payload),
            subject_suffix: frame.subject_suffix,
        }
    }
}

fn payload_text(bytes: &Bytes) -> String {
    std::str::from_utf8(bytes).map_or_else(|_| format!("base64:{}", STANDARD.encode(bytes)), str::to_string)
}

#[cfg(test)]
mod tests;
