use super::*;

#[test]
fn round_trips_offset() {
    let token = encode_page_token(42);
    assert_eq!(decode_page_token(&token).unwrap(), 42);
}

#[test]
fn rejects_invalid_token() {
    assert!(matches!(
        decode_page_token("not-a-token"),
        Err(PageTokenError::InvalidEncoding)
    ));
}

#[test]
fn rejects_unsupported_version() {
    let payload = r#"{"offset":0,"version":2}"#;
    let token = URL_SAFE_NO_PAD.encode(payload);
    assert!(matches!(
        decode_page_token(&token),
        Err(PageTokenError::UnsupportedVersion)
    ));
}

#[test]
fn rejects_invalid_json_payload() {
    let token = URL_SAFE_NO_PAD.encode("not-json");
    assert!(matches!(
        decode_page_token(&token),
        Err(PageTokenError::InvalidPayload(_))
    ));
}

#[test]
fn round_trips_zero_offset() {
    let token = encode_page_token(0);
    assert_eq!(decode_page_token(&token).unwrap(), 0);
}
