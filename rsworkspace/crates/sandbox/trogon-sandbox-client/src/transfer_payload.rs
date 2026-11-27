use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::de::Error as _;
use trogon_sandbox_contract::Base64Data;

use crate::TransportError;

/// Wraps raw bytes as the wire's base64 payload.
pub(crate) fn encode(bytes: &[u8]) -> Result<Base64Data, TransportError> {
    Base64Data::new(STANDARD.encode(bytes)).map_err(|e| TransportError::Encode(serde_json::Error::custom(e)))
}

/// Unwraps a base64 payload from a reply into raw bytes.
pub(crate) fn decode(data: &Base64Data) -> Result<Vec<u8>, TransportError> {
    STANDARD
        .decode(data.as_str())
        .map_err(|e| TransportError::Decode(serde_json::Error::custom(e)))
}
