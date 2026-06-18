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
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use std::error::Error;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::source::standard_webhooks::sign_for_test;

    fn test_secret_string() -> String {
        ["whsec_", "dGVzdC1zZWNyZXQ="].concat()
    }

    fn sign(secret: &IncidentioSigningSecret, webhook_id: &str, timestamp: &str, body: &[u8]) -> String {
        sign_for_test(secret.as_bytes(), webhook_id, timestamp, body)
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
