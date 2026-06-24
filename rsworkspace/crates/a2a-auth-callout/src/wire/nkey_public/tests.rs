use super::*;

fn fresh_account_public() -> String {
    KeyPair::new_account().public_key()
}

#[test]
fn parse_round_trips_through_as_str_display_debug() {
    let raw = fresh_account_public();
    let np = NkeyPublic::parse(raw.clone()).unwrap();
    assert_eq!(np.as_str(), raw);
    assert_eq!(np.to_string(), raw);
    assert!(format!("{np:?}").starts_with("NkeyPublic"));
}

#[test]
fn parse_trims_surrounding_whitespace() {
    let raw = fresh_account_public();
    let padded = format!("  {raw}\n");
    assert_eq!(NkeyPublic::parse(padded).unwrap().as_str(), raw);
}

#[test]
fn parse_rejects_empty_input() {
    let e = NkeyPublic::parse("   ").unwrap_err();
    assert!(matches!(e, AuthCalloutError::WireFormat(_)));
}

#[test]
fn parse_rejects_garbage() {
    let e = NkeyPublic::parse("not-an-nkey").unwrap_err();
    assert!(matches!(e, AuthCalloutError::WireFormat(_)));
}

#[test]
fn verify_jwt_issuer_accepts_valid_token() {
    use crate::wire::test_encode::signed_auth_request;

    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |_| {});
    let claims = server_pub.verify_jwt_issuer(&token).unwrap();
    assert_eq!(claims.nats.user_nkey, user.public_key());
}

#[test]
fn verify_jwt_issuer_rejects_mismatched_server_issuer() {
    use crate::wire::test_encode::signed_auth_request;

    let server = KeyPair::new_account();
    let other_pub = NkeyPublic::parse(KeyPair::new_account().public_key()).unwrap();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |_| {});
    let err = other_pub.verify_jwt_issuer(&token).unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}

#[test]
fn verify_jwt_issuer_rejects_invalid_segment_count() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let err = server_pub.verify_jwt_issuer("only-one-segment").unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}

#[test]
fn verify_jwt_issuer_rejects_wrong_audience() {
    use crate::wire::test_encode::signed_auth_request;

    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |c| {
        c.aud = Some("wrong-audience".into());
    });
    let err = server_pub.verify_jwt_issuer(&token).unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}
