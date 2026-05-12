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

use super::IncidentioSigningSecret;
use super::constants::{HEADER_WEBHOOK_ID, HEADER_WEBHOOK_SIGNATURE, HEADER_WEBHOOK_TIMESTAMP};

type HmacSha256 = Hmac<Sha256>;

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
            Self::InvalidWebhookId(err) => Some(err),
            Self::InvalidTimestamp(err) => Some(err),
            Self::InvalidSignatureEncoding(err) => Some(err),
            _ => None,
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
            Self::Invalid(err) => Some(err),
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

pub fn verify(
    headers: &HeaderMap,
    body: &[u8],
    secret: &IncidentioSigningSecret,
    timestamp_tolerance: NonZeroDuration,
) -> Result<VerifiedWebhook, SignatureError> {
    let webhook_id =
        WebhookId::new(header_str(headers, HEADER_WEBHOOK_ID)?).map_err(SignatureError::InvalidWebhookId)?;
    let webhook_timestamp = WebhookTimestamp::new(header_str(headers, HEADER_WEBHOOK_TIMESTAMP)?)
        .map_err(SignatureError::InvalidTimestamp)?;
    let signature_header = header_str(headers, HEADER_WEBHOOK_SIGNATURE)?;

    verify_timestamp(&webhook_timestamp, timestamp_tolerance)?;
    verify_signature(
        secret,
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
    secret: &IncidentioSigningSecret,
    webhook_id: &str,
    webhook_timestamp: &str,
    body: &[u8],
    signature_header: &str,
) -> Result<(), SignatureError> {
    let signed_content = signed_content(webhook_id, webhook_timestamp, body);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC-SHA256 accepts any key length");
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
            Err(err) => {
                if invalid_v1.is_none() {
                    invalid_v1 = Some(err);
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
    let err = invalid_v1.expect("v1 signature entries without decodable signatures must be invalid");
    Err(SignatureError::InvalidSignatureEncoding(err))
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
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::error::Error;

    fn test_secret_string() -> String {
        ["whsec_", "dGVzdC1zZWNyZXQ="].concat()
    }

    fn sign(secret: &IncidentioSigningSecret, webhook_id: &str, timestamp: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&signed_content(webhook_id, timestamp, body));
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        format!("v1,{signature}")
    }

    fn test_secret() -> IncidentioSigningSecret {
        IncidentioSigningSecret::new(test_secret_string()).unwrap()
    }

    fn valid_timestamp() -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn webhook_headers(body: &[u8]) -> HeaderMap {
        let webhook_id = "msg_123";
        let timestamp = valid_timestamp();
        let secret = test_secret();
        let signature = sign(&secret, webhook_id, &timestamp, body);

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static("msg_123"));
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_str(&timestamp).unwrap());
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_str(&signature).unwrap());
        headers
    }

    #[test]
    fn valid_webhook_headers_pass() {
        let body = br#"{"event_type":"public_incident.incident_created_v2"}"#;
        let headers = webhook_headers(body);
        let verified = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap();

        assert_eq!(verified.webhook_id.as_str(), "msg_123");
        assert!(verified.webhook_timestamp.as_secs() > 0);
    }

    #[test]
    fn webhook_id_round_trips_through_value_object_apis() {
        let webhook_id = WebhookId::new("msg_123").unwrap();

        assert_eq!(webhook_id.as_str(), "msg_123");
        assert_eq!(webhook_id.as_ref(), "msg_123");
        assert_eq!(webhook_id.to_string(), "msg_123");
    }

    #[test]
    fn webhook_id_empty_error_has_no_source() {
        let err = WebhookId::new("").unwrap_err();

        assert_eq!(err.to_string(), "webhook id must not be empty");
        assert!(err.source().is_none());
    }

    #[test]
    fn webhook_timestamp_round_trips_through_value_object_apis() {
        let webhook_timestamp = WebhookTimestamp::new("123").unwrap();

        assert_eq!(webhook_timestamp.as_str(), "123");
        assert_eq!(webhook_timestamp.as_ref(), "123");
        assert_eq!(webhook_timestamp.as_secs(), 123);
        assert_eq!(webhook_timestamp.to_string(), "123");
    }

    #[test]
    fn webhook_timestamp_invalid_error_preserves_source() {
        let err = WebhookTimestamp::new("not-a-number").unwrap_err();

        assert_eq!(err.to_string(), "webhook timestamp must be an integer");
        assert!(err.source().is_some());
    }

    #[test]
    fn webhook_timestamp_empty_error_has_no_source() {
        let err = WebhookTimestamp::new("").unwrap_err();

        assert_eq!(err.to_string(), "webhook timestamp must not be empty");
        assert!(err.source().is_none());
    }

    #[test]
    fn missing_header_trio_fails() {
        let headers = HeaderMap::new();
        let err = verify(
            &headers,
            br#"{}"#,
            &test_secret(),
            NonZeroDuration::from_secs(300).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::MissingHeaders));
        assert_eq!(err.to_string(), "missing required signature headers");
        assert!(err.source().is_none());
    }

    #[test]
    fn unsupported_svix_headers_fail() {
        let body = br#"{}"#;
        let timestamp = valid_timestamp();
        let secret = test_secret();
        let signature = sign(&secret, "msg_svix", &timestamp, body);
        let mut headers = HeaderMap::new();
        headers.insert("svix-id", HeaderValue::from_static("msg_svix"));
        headers.insert("svix-timestamp", HeaderValue::from_str(&timestamp).unwrap());
        headers.insert("svix-signature", HeaderValue::from_str(&signature).unwrap());
        let err = verify(&headers, body, &secret, NonZeroDuration::from_secs(300).unwrap()).unwrap_err();
        assert!(matches!(err, SignatureError::MissingHeaders));
    }

    #[test]
    fn malformed_timestamp_fails() {
        let body = br#"{}"#;
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_static("not-a-number"));
        let err = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();
        assert!(matches!(err, SignatureError::InvalidTimestamp(_)));
        assert_eq!(err.to_string(), "invalid webhook timestamp");
        assert!(err.source().is_some());
    }

    #[test]
    fn empty_webhook_id_fails() {
        let body = br#"{}"#;
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static(""));

        let err = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert!(matches!(err, SignatureError::InvalidWebhookId(WebhookIdError::Empty)));
        assert_eq!(err.to_string(), "invalid webhook id");
        assert!(err.source().is_some());
    }

    #[test]
    fn empty_timestamp_fails() {
        let body = br#"{}"#;
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_static(""));

        let err = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert!(matches!(
            err,
            SignatureError::InvalidTimestamp(WebhookTimestampError::Empty)
        ));
        assert_eq!(err.to_string(), "invalid webhook timestamp");
        assert!(err.source().is_some());
    }

    #[test]
    fn invalid_header_value_preserves_source_error() {
        let body = br#"{}"#;
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_bytes(b"\xFF").unwrap());

        let err = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert!(matches!(
            &err,
            SignatureError::InvalidHeaderValue {
                name: HEADER_WEBHOOK_ID,
                ..
            }
        ));
        assert_eq!(err.to_string(), "invalid value for header webhook-id");
        assert!(err.source().is_some());
    }

    #[test]
    fn stale_timestamp_fails() {
        let body = br#"{}"#;
        let secret = test_secret();
        let timestamp = "1";
        let signature = sign(&secret, "msg_123", timestamp, body);

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static("msg_123"));
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_static("1"));
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_str(&signature).unwrap());

        let err = verify(&headers, body, &secret, NonZeroDuration::from_secs(300).unwrap()).unwrap_err();
        assert!(matches!(err, SignatureError::StaleTimestamp));
        assert_eq!(err.to_string(), "webhook timestamp outside tolerance");
        assert!(err.source().is_none());
    }

    #[test]
    fn single_valid_v1_signature_passes() {
        let body = br#"{}"#;
        let headers = webhook_headers(body);
        assert!(verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap(),).is_ok());
    }

    #[test]
    fn multiple_signatures_with_one_match_pass() {
        let body = br#"{}"#;
        let secret = test_secret();
        let timestamp = valid_timestamp();
        let signature = sign(&secret, "msg_123", &timestamp, body);
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static("msg_123"));
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_str(&timestamp).unwrap());
        headers.insert(
            HEADER_WEBHOOK_SIGNATURE,
            HeaderValue::from_str(&format!("v0,ignored {} ", signature)).unwrap(),
        );

        assert!(verify(&headers, body, &secret, NonZeroDuration::from_secs(300).unwrap(),).is_ok());
    }

    #[test]
    fn invalid_base64_signature_entry_fails() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static("msg_123"));
        headers.insert(
            HEADER_WEBHOOK_TIMESTAMP,
            HeaderValue::from_str(&valid_timestamp()).unwrap(),
        );
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_static("v1,not-base64!"));

        let err = verify(
            &headers,
            br#"{}"#,
            &test_secret(),
            NonZeroDuration::from_secs(300).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SignatureError::InvalidSignatureEncoding(_)));
        assert_eq!(err.to_string(), "invalid signature encoding");
        assert!(err.source().is_some());
    }

    #[test]
    fn missing_v1_signature_fails() {
        let body = br#"{}"#;
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_static("v0,ignored"));

        let err = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert!(matches!(err, SignatureError::MissingV1Signature));
        assert_eq!(err.to_string(), "missing v1 signature");
        assert!(err.source().is_none());
    }

    #[test]
    fn malformed_signature_entries_without_commas_are_ignored() {
        let body = br#"{}"#;
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_static("nonsense"));

        let err = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert!(matches!(err, SignatureError::MissingV1Signature));
    }

    #[test]
    fn mismatched_signature_fails() {
        let body = br#"{}"#;
        let mut headers = webhook_headers(body);
        headers.insert(
            HEADER_WEBHOOK_SIGNATURE,
            HeaderValue::from_static("v1,d3JvbmdzaWduYXR1cmU="),
        );
        let err = verify(&headers, body, &test_secret(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();
        assert!(matches!(err, SignatureError::Mismatch));
        assert_eq!(err.to_string(), "signature mismatch");
        assert!(err.source().is_none());
    }
}
