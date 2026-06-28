//! Deterministic opaque pagination tokens.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// Error returned when decoding a pagination token fails.
#[derive(Debug, thiserror::Error)]
pub enum PageTokenError {
    #[error("page token is not valid base64")]
    InvalidEncoding,
    #[error("page token payload is invalid")]
    InvalidPayload(#[from] serde_json::Error),
    #[error("page token version is unsupported")]
    UnsupportedVersion,
}

/// Encoded pagination cursor for deterministic registry paging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct PageTokenPayload {
    version: u8,
    offset: u64,
}

const PAGE_TOKEN_VERSION: u8 = 1;

/// Encode a byte offset into an opaque page token.
pub fn encode_page_token(offset: u64) -> String {
    let payload = format!(r#"{{"offset":{offset},"version":{PAGE_TOKEN_VERSION}}}"#);
    URL_SAFE_NO_PAD.encode(payload)
}

/// Decode an opaque page token into a byte offset.
pub fn decode_page_token(token: &str) -> Result<u64, PageTokenError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(token.trim())
        .map_err(|_| PageTokenError::InvalidEncoding)?;
    let payload: PageTokenPayload = serde_json::from_slice(&bytes)?;
    if payload.version != PAGE_TOKEN_VERSION {
        return Err(PageTokenError::UnsupportedVersion);
    }
    Ok(payload.offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_offset() {
        let token = encode_page_token(42);
        assert_eq!(decode_page_token(&token).unwrap(), 42);
    }

    #[test]
    fn rejects_invalid_token() {
        assert!(matches!(
            decode_page_token("not-a-token"),
            Err(PageTokenError::InvalidEncoding)
        ));
    }
}
