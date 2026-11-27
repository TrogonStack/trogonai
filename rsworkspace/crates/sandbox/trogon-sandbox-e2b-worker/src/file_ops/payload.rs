use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::Base64Data;
use trogon_sandbox_file_ops::FileOpError;

/// The reply payload carrying `bytes`.
pub(super) fn encode(bytes: &[u8]) -> Result<Base64Data, FileOpError> {
    Ok(Base64Data::new(STANDARD.encode(bytes))?)
}

/// The bytes `data` holds. A `Base64Data` was decoded once when it was parsed.
pub(super) fn decode(data: &Base64Data) -> Vec<u8> {
    STANDARD.decode(data.as_str()).unwrap_or_default()
}
