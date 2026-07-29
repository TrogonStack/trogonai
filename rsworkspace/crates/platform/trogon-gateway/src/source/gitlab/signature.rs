use axum::http::HeaderMap;
use trogon_std::NonZeroDuration;

use super::GitLabSigningToken;
use super::constants::HEADER_NAMES;
use crate::source::standard_webhooks;

pub use crate::source::standard_webhooks::SignatureError;
#[cfg(test)]
pub use crate::source::standard_webhooks::{WebhookId, WebhookTimestamp};

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
