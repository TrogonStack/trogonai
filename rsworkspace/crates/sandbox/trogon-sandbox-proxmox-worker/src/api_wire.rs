use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::constants::MAX_RESPONSE_BYTES;
use crate::{ProxmoxApiError, VmId, VmIdError};

/// Every Proxmox reply wraps its payload in `data`.
#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    pub(crate) data: T,
}

/// Proxmox reports ids as numbers or as decimal strings.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum VmIdResponse {
    Integer(u32),
    Decimal(String),
}

impl VmIdResponse {
    pub(crate) fn validated(self) -> Result<VmId, VmIdError> {
        match self {
            Self::Integer(value) => VmId::try_from(value),
            Self::Decimal(value) => value.parse(),
        }
    }
}

/// Reads a reply body within its byte budget and decodes it.
pub(crate) async fn decode_response<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ProxmoxApiError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ProxmoxApiError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(ProxmoxApiError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Whether a guest-agent failure message says the file does not exist.
pub(crate) fn is_missing_file_error(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("no such file") || detail.contains("enoent")
}

#[cfg(test)]
mod tests;
