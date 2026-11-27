use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::Base64Data;

use crate::CheckFailureError;

pub(crate) fn encode(bytes: &[u8]) -> Result<Base64Data, CheckFailureError> {
    Base64Data::new(STANDARD.encode(bytes)).map_err(CheckFailureError::identifier)
}

pub(crate) fn decode(data: &Base64Data) -> Result<Vec<u8>, CheckFailureError> {
    STANDARD.decode(data.as_str()).map_err(CheckFailureError::identifier)
}

pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
