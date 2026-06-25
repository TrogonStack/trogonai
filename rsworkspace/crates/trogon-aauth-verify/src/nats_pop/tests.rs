use super::*;

#[test]
fn content_digest_is_stable() {
    let a = content_digest_sha256(b"hello");
    let b = content_digest_sha256(b"hello");
    assert_eq!(a, b);
    assert!(a.starts_with("sha-256=:"));
}

#[test]
fn nats_headers_lookup_is_case_insensitive() {
    let items = vec![("AAuth-Token".to_string(), "tok".to_string())];
    let headers = NatsHeaders::new(&items);
    assert_eq!(headers.get("aauth-token"), Some("tok"));
    assert_eq!(headers.get("AAUTH-TOKEN"), Some("tok"));
    assert_eq!(headers.get("missing"), None);
}

#[test]
fn nats_headers_debug_reports_len() {
    let items = vec![("a".to_string(), "1".to_string()), ("b".to_string(), "2".to_string())];
    let headers = NatsHeaders::new(&items);
    let s = format!("{headers:?}");
    assert!(s.contains("len: 2"), "got {s}");
}

#[test]
fn nats_pop_error_display_strings_are_distinct() {
    let cases = [
        format!("{}", NatsPopError::MissingHeader("X-Foo")),
        format!("{}", NatsPopError::DigestMismatch),
        format!("{}", NatsPopError::Skew),
        format!("{}", NatsPopError::Replay),
        format!("{}", NatsPopError::BadSignature),
    ];
    for window in cases.windows(2) {
        assert_ne!(window[0], window[1]);
    }
}

#[test]
fn nats_headers_clone_preserves_view() {
    let items = vec![("a".to_string(), "1".to_string())];
    let h1 = NatsHeaders::new(&items);
    let h2 = h1.clone();
    assert_eq!(h2.get("a"), Some("1"));
}

#[tokio::test(flavor = "current_thread")]
async fn verify_returns_skew_when_clock_outside_window() {
    use crate::jwks::StaticJwks;
    use crate::replay::InMemoryReplayStore;
    use crate::time_source::SystemTimeSource;

    let headers_vec: Vec<(String, String)> = vec![
        (headers::NATS_TOKEN.to_string(), "tok".into()),
        (headers::NATS_SIG_INPUT.to_string(), "".into()),
        (headers::NATS_SIG.to_string(), "".into()),
        // Far in the past so skew check trips before token verification.
        (headers::NATS_SIG_CREATED.to_string(), "0".into()),
        (headers::NATS_SIG_NONCE.to_string(), "n1".into()),
    ];
    let req = NatsRequest {
        subject: "s",
        reply: None,
        payload: b"",
        headers: NatsHeaders::new(&headers_vec),
    };
    let verifier = NatsPopVerifier::new(StaticJwks::new(), SystemTimeSource, InMemoryReplayStore::new());
    let err = verifier.verify(&req).await.unwrap_err();
    assert!(matches!(err, NatsPopError::Skew), "got {err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn verify_returns_invalid_header_value_for_unparseable_created() {
    use crate::jwks::StaticJwks;
    use crate::replay::InMemoryReplayStore;
    use crate::time_source::SystemTimeSource;
    let headers_vec: Vec<(String, String)> = vec![
        (headers::NATS_TOKEN.to_string(), "tok".into()),
        (headers::NATS_SIG_INPUT.to_string(), "".into()),
        (headers::NATS_SIG.to_string(), "".into()),
        // Non-integer value — drives the typed ParseIntError source path.
        (headers::NATS_SIG_CREATED.to_string(), "not-an-int".into()),
        (headers::NATS_SIG_NONCE.to_string(), "n1".into()),
    ];
    let req = NatsRequest {
        subject: "s",
        reply: None,
        payload: b"",
        headers: NatsHeaders::new(&headers_vec),
    };
    let verifier = NatsPopVerifier::new(StaticJwks::new(), SystemTimeSource, InMemoryReplayStore::new());
    let err = verifier.verify(&req).await.unwrap_err();
    assert!(
        matches!(
            err,
            NatsPopError::InvalidHeaderValue {
                header,
                ..
            } if header == headers::NATS_SIG_CREATED
        ),
        "got {err:?}"
    );
}

#[test]
fn invalid_confirmation_key_unsupported_alg_surfaces() {
    let jwk = serde_json::json!({"kty": "oct", "k": "AA"});
    let err = verify_signature_with_jwk(&jwk, b"base", "sig").unwrap_err();
    assert!(matches!(
        err,
        NatsPopError::InvalidConfirmationKey(InvalidConfirmationKey::UnsupportedAlgorithm)
    ));
}

#[test]
fn invalid_confirmation_key_bad_shape_surfaces_deserialize_source() {
    // Missing required fields cause serde_json::from_value -> Err.
    let jwk = serde_json::json!({"kty": "EC"});
    let err = verify_signature_with_jwk(&jwk, b"base", "sig").unwrap_err();
    assert!(matches!(
        err,
        NatsPopError::InvalidConfirmationKey(InvalidConfirmationKey::Deserialize(_))
    ));
}
