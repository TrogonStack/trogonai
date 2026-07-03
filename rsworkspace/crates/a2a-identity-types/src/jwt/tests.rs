use super::*;
use serde_json::json;

fn encode_segment(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
}

fn build_token(payload: Value) -> String {
    let header = encode_segment(&json!({ "alg": "HS256", "typ": "JWT" }));
    let body = encode_segment(&payload);
    format!("{header}.{body}.signature")
}

#[test]
fn header_value_parses_compact_jwt() {
    let value = CallerJwtHeaderValue::parse("a.b.c").unwrap();
    assert_eq!(value.as_str(), "a.b.c");
}

#[test]
fn header_value_rejects_empty() {
    let err = CallerJwtHeaderValue::parse("   ").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn header_value_rejects_non_compact() {
    let err = CallerJwtHeaderValue::parse("a.b").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn header_value_redacts_on_display() {
    let value = CallerJwtHeaderValue::parse("a.b.c").unwrap();
    assert_eq!(format!("{value}"), "<redacted>");
}

#[test]
fn header_value_from_minted_copies_string() {
    let minted = MintedUserJwt::new("a.b.c").unwrap();
    let value = CallerJwtHeaderValue::from_minted(&minted);
    assert_eq!(value.as_str(), "a.b.c");
}

#[test]
fn minted_jwt_into_string_yields_owned_token() {
    let minted = MintedUserJwt::new("a.b.c").unwrap();
    assert_eq!(minted.clone().into_string(), "a.b.c");
    assert_eq!(minted.as_str(), "a.b.c");
}

#[test]
fn minted_jwt_rejects_non_compact_input() {
    assert!(matches!(MintedUserJwt::new("a.b").unwrap_err(), JwtError::Decode(_)));
    assert!(matches!(MintedUserJwt::new("a..c").unwrap_err(), JwtError::Decode(_)));
    assert!(matches!(MintedUserJwt::new("a.b.").unwrap_err(), JwtError::Decode(_)));
    assert!(matches!(MintedUserJwt::new("").unwrap_err(), JwtError::Decode(_)));
}

#[test]
fn parse_rejects_empty_segments() {
    assert!(matches!(
        CallerJwtHeaderValue::parse("a..c").unwrap_err(),
        JwtError::Decode(_)
    ));
    assert!(matches!(
        CallerJwtHeaderValue::parse("a.b.").unwrap_err(),
        JwtError::Decode(_)
    ));
    assert!(matches!(
        CallerJwtHeaderValue::parse(".b.c").unwrap_err(),
        JwtError::Decode(_)
    ));
}

#[test]
fn decode_payload_returns_value_for_valid_segments() {
    let token = build_token(json!({ "exp": 9_999_999_999i64 }));
    let payload = decode_nats_user_payload(&token).unwrap();
    assert_eq!(payload["exp"], json!(9_999_999_999i64));
}

#[test]
fn decode_payload_rejects_wrong_segment_count() {
    let err = decode_nats_user_payload("a.b").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn decode_payload_rejects_invalid_base64() {
    let err = decode_nats_user_payload("header.!!!.sig").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn decode_payload_rejects_non_json_body() {
    let body = URL_SAFE_NO_PAD.encode(b"not-json");
    let err = decode_nats_user_payload(&format!("header.{body}.sig")).unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn ensure_fresh_accepts_future_exp() {
    let token = build_token(json!({ "exp": 9_999_999_999i64 }));
    MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap();
}

#[test]
fn ensure_fresh_rejects_missing_exp() {
    let token = build_token(json!({}));
    let err = MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap_err();
    assert!(matches!(err, JwtError::Decode(ref msg) if msg.contains("missing exp")));
}

#[test]
fn ensure_fresh_rejects_expired_exp() {
    let token = build_token(json!({ "exp": 1i64 }));
    let err = MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap_err();
    assert!(matches!(err, JwtError::Decode(ref msg) if msg.contains("expired")));
}

#[test]
fn ensure_fresh_rejects_future_nbf() {
    let token = build_token(json!({ "exp": 9_999_999_999i64, "nbf": 9_999_999_998i64 }));
    let err = MintedUserJwt::new(token).unwrap().ensure_fresh().unwrap_err();
    assert!(matches!(err, JwtError::Decode(ref msg) if msg.contains("not yet valid")));
}
