use axum::http::HeaderMap;
use trogon_std::NonZeroDuration;

use super::IncidentioSigningSecret;
use super::constants::HEADER_NAMES;
use crate::source::standard_webhooks;

pub use crate::source::standard_webhooks::{SignatureError, VerifiedWebhook};
#[cfg(test)]
pub use crate::source::standard_webhooks::{WebhookId, WebhookIdError, WebhookTimestamp, WebhookTimestampError};

pub fn verify(
    headers: &HeaderMap,
    body: &[u8],
    secret: &IncidentioSigningSecret,
    timestamp_tolerance: NonZeroDuration,
) -> Result<VerifiedWebhook, SignatureError> {
    standard_webhooks::verify(headers, body, secret.as_bytes(), timestamp_tolerance, HEADER_NAMES)
}

#[cfg(test)]
mod tests;
