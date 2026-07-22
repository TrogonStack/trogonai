use std::error::Error;

use super::*;

#[test]
fn max_payload_subtracts_overhead() {
    let mp = MaxPayload::from_server_limit(1_048_576);
    assert_eq!(mp.threshold(), 1_048_576 - PROTOCOL_OVERHEAD);
}

#[test]
fn max_payload_saturates_at_zero() {
    let mp = MaxPayload::from_server_limit(100);
    assert_eq!(mp.threshold(), 0);
}

#[test]
fn is_claim_returns_true_for_v1_header() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
    assert!(is_claim(&headers));
}

#[test]
fn is_claim_returns_false_for_missing_header() {
    let headers = HeaderMap::new();
    assert!(!is_claim(&headers));
}

#[test]
fn is_claim_returns_false_for_wrong_version() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CLAIM_CHECK, "v99");
    assert!(!is_claim(&headers));
}

#[test]
fn claim_object_key_contains_subject() {
    let key = claim_object_key("github.push");
    assert!(key.starts_with("github.push/"));
}

#[test]
fn strip_claim_headers_removes_claim_prefix() {
    let mut headers = HeaderMap::new();
    headers.insert("X-Custom", "keep");
    headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
    headers.insert(HEADER_CLAIM_BUCKET, "bucket");
    headers.insert(HEADER_CLAIM_KEY, "key");

    let stripped = strip_claim_headers(headers);
    assert!(stripped.get(HEADER_CLAIM_CHECK).is_none());
    assert!(stripped.get(HEADER_CLAIM_BUCKET).is_none());
    assert!(stripped.get(HEADER_CLAIM_KEY).is_none());
    assert_eq!(stripped.get("X-Custom").unwrap().as_str(), "keep");
}

#[test]
fn strip_claim_headers_noop_when_clean() {
    let mut headers = HeaderMap::new();
    headers.insert("X-Custom", "value");

    let stripped = strip_claim_headers(headers);
    assert_eq!(stripped.get("X-Custom").unwrap().as_str(), "value");
}

#[test]
fn claim_resolve_error_display_missing_key() {
    let err: ClaimResolveError<String> = ClaimResolveError::MissingKey;
    let msg = err.to_string();
    assert!(msg.contains(HEADER_CLAIM_KEY));
}

#[test]
fn claim_resolve_error_display_store_failed() {
    let err: ClaimResolveError<String> = ClaimResolveError::StoreFailed("connection refused".to_string());
    let msg = err.to_string();
    assert!(msg.contains("connection refused"));
}

#[test]
fn claim_resolve_error_display_read_failed() {
    let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
    let err: ClaimResolveError<String> = ClaimResolveError::ReadFailed(io_err);
    let msg = err.to_string();
    assert!(msg.contains("pipe broke"));
}

#[test]
fn claim_resolve_error_source_missing_key() {
    let err: ClaimResolveError<std::io::Error> = ClaimResolveError::MissingKey;
    assert!(err.source().is_none());
}

#[test]
fn claim_resolve_error_source_store_failed() {
    let inner = std::io::Error::other("boom");
    let err: ClaimResolveError<std::io::Error> = ClaimResolveError::StoreFailed(inner);
    assert!(err.source().is_some());
}

#[test]
fn claim_resolve_error_source_read_failed() {
    let inner = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
    let err: ClaimResolveError<std::io::Error> = ClaimResolveError::ReadFailed(inner);
    assert!(err.source().is_some());
}

#[test]
fn max_payload_limit_trait_delegates_to_inner() {
    let limit = MaxPayload::from_server_limit(5_000);
    assert_eq!(limit.max_payload().threshold(), limit.threshold());
}

#[test]
fn claim_check_publisher_debug_formats() {
    #[derive(Debug)]
    struct MockPublisher;
    #[derive(Debug)]
    struct MockStore;

    let publisher = ClaimCheckPublisher::new(
        MockPublisher,
        MockStore,
        "claims".into(),
        MaxPayload::from_server_limit(1_024),
    );
    let debug = format!("{publisher:?}");
    assert!(debug.contains("ClaimCheckPublisher"));
    assert!(debug.contains("claims"));
}
