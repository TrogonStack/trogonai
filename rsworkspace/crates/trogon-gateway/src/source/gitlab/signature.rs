use axum::http::HeaderMap;
use trogon_std::NonZeroDuration;

use super::GitLabSigningToken;
use super::constants::{HEADER_WEBHOOK_ID, HEADER_WEBHOOK_SIGNATURE, HEADER_WEBHOOK_TIMESTAMP};
use crate::source::standard_webhooks::{self, HeaderNames};

pub use crate::source::standard_webhooks::SignatureError;
#[cfg(test)]
pub use crate::source::standard_webhooks::{WebhookId, WebhookTimestamp};

const HEADER_NAMES: HeaderNames = HeaderNames {
    webhook_id: HEADER_WEBHOOK_ID,
    webhook_timestamp: HEADER_WEBHOOK_TIMESTAMP,
    webhook_signature: HEADER_WEBHOOK_SIGNATURE,
};

pub fn verify(
    headers: &HeaderMap,
    body: &[u8],
    signing_token: &GitLabSigningToken,
    timestamp_tolerance: NonZeroDuration,
) -> Result<(), SignatureError> {
    standard_webhooks::verify(
        headers,
        body,
        signing_token.as_bytes(),
        timestamp_tolerance,
        HEADER_NAMES,
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests;
