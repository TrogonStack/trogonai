//! Standard Webhooks HMAC-SHA256 `v1` verification shared by webhook sources.
//!
//! This stays local instead of using the upstream `standardwebhooks` crate
//! because gateway sources need configurable timestamp tolerances, raw-byte
//! payload verification, and parsed delivery metadata. The upstream Rust crate
//! currently fixes the tolerance at five minutes, signs through UTF-8 payload
//! conversion, and returns only verification success.
//!
//! TODO: Follow up with the upstream `standardwebhooks` crate. When this module
//! was added, `standardwebhooks` 1.0.2 could not replace it because verification
//! used a fixed five-minute tolerance, signing required UTF-8 payload
//! conversion, and verification returned no delivery metadata. Ask upstream
//! whether configurable tolerance, raw-byte payload verification, and access to
//! the parsed `webhook-id` / `webhook-timestamp` belong in the shared crate. If
//! they do, migrate GitLab and incident.io to the crate and remove this module.

use std::fmt;
use std::num::ParseIntError;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Copy)]
pub(crate) struct HeaderNames {
    pub(crate) webhook_id: &'static str,
    pub(crate) webhook_timestamp: &'static str,
    pub(crate) webhook_signature: &'static str,
}

#[derive(Debug)]
pub enum SignatureError {
    MissingHeaders,
    InvalidHeaderValue {
        name: &'static str,
        source: axum::http::header::ToStrError,
    },
    InvalidWebhookId(WebhookIdError),
    InvalidTimestamp(WebhookTimestampError),
    StaleTimestamp,
    InvalidSignatureEncoding(base64::DecodeError),
    MissingV1Signature,
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeaders => f.write_str("missing required signature headers"),
            Self::InvalidHeaderValue { name, .. } => {
                write!(f, "invalid value for header {name}")
            }
            Self::InvalidWebhookId(_) => f.write_str("invalid webhook id"),
            Self::InvalidTimestamp(_) => f.write_str("invalid webhook timestamp"),
            Self::StaleTimestamp => f.write_str("webhook timestamp outside tolerance"),
            Self::InvalidSignatureEncoding(_) => f.write_str("invalid signature encoding"),
            Self::MissingV1Signature => f.write_str("missing v1 signature"),
            Self::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidHeaderValue { source, .. } => Some(source),
            Self::InvalidWebhookId(error) => Some(error),
            Self::InvalidTimestamp(error) => Some(error),
            Self::InvalidSignatureEncoding(error) => Some(error),
            Self::MissingHeaders | Self::StaleTimestamp | Self::MissingV1Signature | Self::Mismatch => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookIdError {
    Empty,
}

impl fmt::Display for WebhookIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("webhook id must not be empty"),
        }
    }
}

impl std::error::Error for WebhookIdError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WebhookId(Arc<str>);

impl WebhookId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, WebhookIdError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(WebhookIdError::Empty);
        }
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WebhookId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for WebhookId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug)]
pub enum WebhookTimestampError {
    Empty,
    Invalid(ParseIntError),
}

impl fmt::Display for WebhookTimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("webhook timestamp must not be empty"),
            Self::Invalid(_) => f.write_str("webhook timestamp must be an integer"),
        }
    }
}

impl std::error::Error for WebhookTimestampError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Empty => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WebhookTimestamp {
    raw: Arc<str>,
    secs: u64,
}

impl WebhookTimestamp {
    pub fn new(value: impl AsRef<str>) -> Result<Self, WebhookTimestampError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(WebhookTimestampError::Empty);
        }
        let secs = value.parse::<u64>().map_err(WebhookTimestampError::Invalid)?;
        Ok(Self {
            raw: value.into(),
            secs,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn as_secs(&self) -> u64 {
        self.secs
    }
}

impl fmt::Display for WebhookTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for WebhookTimestamp {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWebhook {
    pub webhook_id: WebhookId,
    pub webhook_timestamp: WebhookTimestamp,
}

pub(crate) fn verify<K: AsRef<[u8]> + ?Sized>(
    headers: &HeaderMap,
    body: &[u8],
    signing_key: &K,
    timestamp_tolerance: NonZeroDuration,
    header_names: HeaderNames,
) -> Result<VerifiedWebhook, SignatureError> {
    let webhook_id =
        WebhookId::new(header_str(headers, header_names.webhook_id)?).map_err(SignatureError::InvalidWebhookId)?;
    let webhook_timestamp = WebhookTimestamp::new(header_str(headers, header_names.webhook_timestamp)?)
        .map_err(SignatureError::InvalidTimestamp)?;
    let signature_header = header_str(headers, header_names.webhook_signature)?;

    verify_timestamp(&webhook_timestamp, timestamp_tolerance)?;
    verify_signature(
        signing_key.as_ref(),
        webhook_id.as_str(),
        webhook_timestamp.as_str(),
        body,
        signature_header,
    )?;

    Ok(VerifiedWebhook {
        webhook_id,
        webhook_timestamp,
    })
}

fn header_str<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, SignatureError> {
    headers
        .get(name)
        .ok_or(SignatureError::MissingHeaders)?
        .to_str()
        .map_err(|source| SignatureError::InvalidHeaderValue { name, source })
}

fn verify_timestamp(timestamp: &WebhookTimestamp, tolerance: NonZeroDuration) -> Result<(), SignatureError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age_secs = now.abs_diff(timestamp.as_secs());
    let tolerance_secs = Duration::from(tolerance).as_secs();
    if age_secs > tolerance_secs {
        return Err(SignatureError::StaleTimestamp);
    }
    Ok(())
}

fn verify_signature(
    signing_key: &[u8],
    webhook_id: &str,
    webhook_timestamp: &str,
    body: &[u8],
    signature_header: &str,
) -> Result<(), SignatureError> {
    let signed_content = signed_content(webhook_id, webhook_timestamp, body);
    let mut mac = HmacSha256::new_from_slice(signing_key).expect("HMAC-SHA256 accepts any key length");
    mac.update(&signed_content);
    let computed = mac.finalize().into_bytes();

    let mut saw_v1 = false;
    let mut saw_decodable_v1 = false;
    let mut invalid_v1: Option<base64::DecodeError> = None;

    for entry in signature_header.split(' ') {
        let Some((version, encoded_signature)) = entry.split_once(',') else {
            continue;
        };
        if version != "v1" {
            continue;
        }
        saw_v1 = true;
        match STANDARD.decode(encoded_signature) {
            Ok(expected) => {
                saw_decodable_v1 = true;
                if expected.as_slice().ct_eq(computed.as_slice()).unwrap_u8() == 1 {
                    return Ok(());
                }
            }
            Err(error) => {
                if invalid_v1.is_none() {
                    invalid_v1 = Some(error);
                }
            }
        }
    }

    if !saw_v1 {
        return Err(SignatureError::MissingV1Signature);
    }
    if saw_decodable_v1 {
        return Err(SignatureError::Mismatch);
    }
    let error = invalid_v1.expect("v1 signature entries without decodable signatures must be invalid");
    Err(SignatureError::InvalidSignatureEncoding(error))
}

fn signed_content(webhook_id: &str, webhook_timestamp: &str, body: &[u8]) -> Vec<u8> {
    let mut content = Vec::with_capacity(webhook_id.len() + webhook_timestamp.len() + body.len() + 2);
    content.extend_from_slice(webhook_id.as_bytes());
    content.push(b'.');
    content.extend_from_slice(webhook_timestamp.as_bytes());
    content.push(b'.');
    content.extend_from_slice(body);
    content
}

#[cfg(test)]
pub(crate) fn sign_for_test(signing_key: &[u8], webhook_id: &str, webhook_timestamp: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(signing_key).expect("HMAC-SHA256 accepts any key length");
    mac.update(&signed_content(webhook_id, webhook_timestamp, body));
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    format!("v1,{signature}")
}
