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
