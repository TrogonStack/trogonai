use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use trogon_std::SecretString;

use crate::headers::single_header;
use crate::{
    AuthenticationError, AuthenticationMode, DeliveryId, RegisteredEndpoint, WebhookSecretLookup, WebhookVerifier,
};

#[derive(Clone)]
pub struct SecretWebhookVerifier<L> {
    lookup: L,
}

impl<L> SecretWebhookVerifier<L> {
    pub fn new(lookup: L) -> Self {
        Self { lookup }
    }

    pub fn lookup(&self) -> &L {
        &self.lookup
    }
}

impl<L: WebhookSecretLookup> WebhookVerifier for SecretWebhookVerifier<L> {
    async fn verify(
        &self,
        endpoint: &RegisteredEndpoint,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), AuthenticationError> {
        let delivery_id = single_header(headers, "webhook-id", 128)
            .and_then(|value| DeliveryId::new(value).ok())
            .ok_or(AuthenticationError::Unauthorized)?;
        match endpoint.registration().authentication_mode() {
            AuthenticationMode::Bearer => {
                let authorization =
                    single_header(headers, "authorization", 4096).ok_or(AuthenticationError::Unauthorized)?;
                let (scheme, token) = authorization.split_once(' ').ok_or(AuthenticationError::Unauthorized)?;
                if !scheme.eq_ignore_ascii_case("Bearer")
                    || token.is_empty()
                    || token.bytes().any(|byte| byte.is_ascii_whitespace())
                {
                    return Err(AuthenticationError::Unauthorized);
                }
                let secret = self.lookup.lookup_secret(endpoint).await?;
                let expected = Sha256::digest(secret.as_str().as_bytes());
                let supplied = Sha256::digest(token.as_bytes());
                if expected.as_slice().ct_eq(supplied.as_slice()).unwrap_u8() == 1 {
                    Ok(())
                } else {
                    Err(AuthenticationError::Unauthorized)
                }
            }
            AuthenticationMode::HmacSha256 => {
                let timestamp =
                    single_header(headers, "webhook-timestamp", 20).ok_or(AuthenticationError::Unauthorized)?;
                if timestamp.is_empty() || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(AuthenticationError::Unauthorized);
                }
                let timestamp_seconds = timestamp
                    .parse::<u64>()
                    .map_err(|_| AuthenticationError::Unauthorized)?;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| AuthenticationError::Unavailable)?
                    .as_secs();
                if now.abs_diff(timestamp_seconds) > endpoint.registration().timestamp_tolerance().as_secs() {
                    return Err(AuthenticationError::Unauthorized);
                }
                let signatures =
                    single_header(headers, "webhook-signature", 4096).ok_or(AuthenticationError::Unauthorized)?;
                let secret = self.lookup.lookup_secret(endpoint).await?;
                verify_signature(&secret, delivery_id.as_str(), timestamp, signatures, body)
            }
        }
    }
}

fn verify_signature(
    secret: &SecretString,
    delivery_id: &str,
    timestamp: &str,
    signatures: &str,
    body: &[u8],
) -> Result<(), AuthenticationError> {
    let encoded = secret
        .as_str()
        .strip_prefix("whsec_")
        .ok_or(AuthenticationError::Unavailable)?;
    let key = STANDARD.decode(encoded).map_err(|_| AuthenticationError::Unavailable)?;
    if key.len() != 32 {
        return Err(AuthenticationError::Unavailable);
    }
    let mut hmac = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| AuthenticationError::Unavailable)?;
    hmac.update(delivery_id.as_bytes());
    hmac.update(b".");
    hmac.update(timestamp.as_bytes());
    hmac.update(b".");
    hmac.update(body);
    let computed = hmac.finalize().into_bytes();
    for candidate in signatures.split_ascii_whitespace().take(16) {
        let Some(encoded) = candidate.strip_prefix("v1,") else {
            continue;
        };
        let Ok(supplied) = STANDARD.decode(encoded) else {
            continue;
        };
        if supplied.as_slice().ct_eq(computed.as_slice()).unwrap_u8() == 1 {
            return Ok(());
        }
    }
    Err(AuthenticationError::Unauthorized)
}
