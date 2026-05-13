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
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use std::error::Error;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::source::standard_webhooks::sign_for_test;

    const TEST_SIGNING_TOKEN: &str = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=";

    fn test_token() -> GitLabSigningToken {
        GitLabSigningToken::new(TEST_SIGNING_TOKEN).unwrap()
    }

    fn valid_timestamp() -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn sign(token: &GitLabSigningToken, webhook_id: &str, timestamp: &str, body: &[u8]) -> String {
        sign_for_test(token.as_bytes(), webhook_id, timestamp, body)
    }

    fn webhook_headers(body: &[u8]) -> HeaderMap {
        let webhook_id = "msg_123";
        let timestamp = valid_timestamp();
        let signature = sign(&test_token(), webhook_id, &timestamp, body);

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static(webhook_id));
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_str(&timestamp).unwrap());
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_str(&signature).unwrap());
        headers
    }

    #[test]
    fn valid_webhook_headers_pass() {
        let body = br#"{"event_name":"push"}"#;
        let headers = webhook_headers(body);

        verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()).unwrap();
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
    fn invalid_timestamp_error_has_source() {
        let err = WebhookTimestamp::new("not-a-number").unwrap_err();

        assert_eq!(err.to_string(), "webhook timestamp must be an integer");
        assert!(err.source().is_some());
    }

    #[test]
    fn empty_timestamp_error_has_no_source() {
        let err = WebhookTimestamp::new("").unwrap_err();

        assert_eq!(err.to_string(), "webhook timestamp must not be empty");
        assert!(err.source().is_none());
    }

    #[test]
    fn invalid_header_value_exposes_source() {
        let body = b"{}";
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_bytes(b"\xff").unwrap());

        let err = verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert_eq!(err.to_string(), "invalid value for header webhook-id");
        assert!(err.source().is_some());
    }

    #[test]
    fn empty_webhook_id_exposes_source() {
        let body = b"{}";
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static(""));

        let err = verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert_eq!(err.to_string(), "invalid webhook id");
        assert!(err.source().is_some());
    }

    #[test]
    fn empty_webhook_timestamp_exposes_source() {
        let body = b"{}";
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_static(""));

        let err = verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert_eq!(err.to_string(), "invalid webhook timestamp");
        assert!(err.source().is_some());
    }

    #[test]
    fn stale_timestamp_fails() {
        let body = b"{}";
        let webhook_id = "msg_123";
        let timestamp = "1";
        let signature = sign(&test_token(), webhook_id, timestamp, body);
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static(webhook_id));
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_static(timestamp));
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_str(&signature).unwrap());

        assert!(matches!(
            verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()),
            Err(SignatureError::StaleTimestamp)
        ));
    }

    #[test]
    fn missing_headers_fail() {
        let headers = HeaderMap::new();

        assert!(matches!(
            verify(&headers, b"{}", &test_token(), NonZeroDuration::from_secs(300).unwrap()),
            Err(SignatureError::MissingHeaders)
        ));
    }

    #[test]
    fn missing_v1_signature_fails() {
        let body = b"{}";
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_static("v0,ignored"));

        assert!(matches!(
            verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()),
            Err(SignatureError::MissingV1Signature)
        ));
    }

    #[test]
    fn invalid_base64_signature_fails() {
        let body = b"{}";
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_static("v1,not-base64!"));

        let err = verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()).unwrap_err();

        assert!(matches!(err, SignatureError::InvalidSignatureEncoding(_)));
        assert_eq!(err.to_string(), "invalid signature encoding");
        assert!(err.source().is_some());
    }

    #[test]
    fn mismatched_signature_fails() {
        let body = b"{}";
        let mut headers = webhook_headers(body);
        headers.insert(HEADER_WEBHOOK_SIGNATURE, HeaderValue::from_static("v1,d3Jvbmc="));

        assert!(matches!(
            verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn malformed_signature_entries_are_ignored() {
        let body = b"{}";
        let mut headers = webhook_headers(body);
        headers.insert(
            HEADER_WEBHOOK_SIGNATURE,
            HeaderValue::from_static("malformed v1,d3Jvbmc="),
        );

        assert!(matches!(
            verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn multiple_signatures_with_one_match_pass() {
        let body = b"{}";
        let webhook_id = "msg_123";
        let timestamp = valid_timestamp();
        let signature = sign(&test_token(), webhook_id, &timestamp, body);
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_WEBHOOK_ID, HeaderValue::from_static(webhook_id));
        headers.insert(HEADER_WEBHOOK_TIMESTAMP, HeaderValue::from_str(&timestamp).unwrap());
        headers.insert(
            HEADER_WEBHOOK_SIGNATURE,
            HeaderValue::from_str(&format!("v1,d3Jvbmc= {signature}")).unwrap(),
        );

        verify(&headers, body, &test_token(), NonZeroDuration::from_secs(300).unwrap()).unwrap();
    }

    #[test]
    fn display_messages_are_stable() {
        assert_eq!(
            SignatureError::MissingHeaders.to_string(),
            "missing required signature headers"
        );
        assert!(SignatureError::MissingHeaders.source().is_none());
        assert_eq!(
            SignatureError::StaleTimestamp.to_string(),
            "webhook timestamp outside tolerance"
        );
        assert!(SignatureError::StaleTimestamp.source().is_none());
        assert_eq!(SignatureError::MissingV1Signature.to_string(), "missing v1 signature");
        assert!(SignatureError::MissingV1Signature.source().is_none());
        assert_eq!(SignatureError::Mismatch.to_string(), "signature mismatch");
        assert!(SignatureError::Mismatch.source().is_none());
    }
}
