use axum::http::HeaderMap;
use trogon_std::NonZeroDuration;

use super::IncidentioSigningSecret;
use super::constants::{HEADER_WEBHOOK_ID, HEADER_WEBHOOK_SIGNATURE, HEADER_WEBHOOK_TIMESTAMP};
use crate::source::standard_webhooks::{self, HeaderNames};

pub use crate::source::standard_webhooks::{SignatureError, VerifiedWebhook};
#[cfg(test)]
pub use crate::source::standard_webhooks::{WebhookId, WebhookIdError, WebhookTimestamp, WebhookTimestampError};

const HEADER_NAMES: HeaderNames = HeaderNames {
    webhook_id: HEADER_WEBHOOK_ID,
    webhook_timestamp: HEADER_WEBHOOK_TIMESTAMP,
    webhook_signature: HEADER_WEBHOOK_SIGNATURE,
};

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
