use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::Base64Data;

use crate::FileOpError;

/// The number of raw bytes `data` decodes to, without decoding it.
pub(crate) fn decoded_len(data: &Base64Data) -> u64 {
    let text = data.as_str();
    let padding = text.bytes().rev().take_while(|byte| *byte == b'=').count();
    (text.len() as u64 / 4 * 3).saturating_sub(padding as u64)
}

/// The bytes `data` holds. A `Base64Data` was decoded once when it was parsed, so this cannot lose
/// anything.
pub(crate) fn decode(data: &Base64Data) -> Vec<u8> {
    STANDARD.decode(data.as_str()).unwrap_or_default()
}

pub(crate) fn encode(bytes: &[u8]) -> Result<Base64Data, FileOpError> {
    Ok(Base64Data::new(STANDARD.encode(bytes))?)
}
