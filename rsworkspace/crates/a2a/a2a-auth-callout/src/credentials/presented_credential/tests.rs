use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;

fn compact(header: &str) -> String {
    format!(
        "{}.{}.c2ln",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(r#"{"sub":"caller"}"#)
    )
}

#[test]
fn a_compact_jws_is_a_bearer_token() {
    let token = compact(r#"{"alg":"RS256","kid":"k1"}"#);
    let credential = PresentedCredential::from_secret(&token).unwrap();
    assert!(matches!(credential, PresentedCredential::BearerToken(bearer) if bearer.as_str() == token));
}

#[test]
fn an_opaque_secret_is_an_api_key() {
    let credential = PresentedCredential::from_secret("k_live_demo").unwrap();
    assert!(matches!(credential, PresentedCredential::ApiKey(key) if key.as_str() == "k_live_demo"));
}

#[test]
fn a_secret_without_a_jose_header_in_three_segments_is_an_api_key() {
    let secrets = [
        "a.b.c".to_owned(),
        compact(r#"{"typ":"JWT"}"#),
        format!("{}.extra", compact(r#"{"alg":"RS256"}"#)),
        format!("{}.only-two", URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#)),
    ];
    for (case, secret) in secrets.iter().enumerate() {
        let credential = PresentedCredential::from_secret(secret).unwrap();
        assert!(
            matches!(credential, PresentedCredential::ApiKey(_)),
            "credential shape case {case} should be classified as an API key"
        );
    }
}

#[test]
fn an_empty_secret_is_refused() {
    assert!(matches!(PresentedCredential::from_secret(""), Err(ApiKeyError::Empty)));
}

#[test]
fn debug_redacts_the_secret() {
    let token = compact(r#"{"alg":"RS256"}"#);
    let bearer = format!("{:?}", PresentedCredential::from_secret(&token).unwrap());
    let api_key = format!("{:?}", PresentedCredential::from_secret("k_live_demo").unwrap());
    assert!(!bearer.contains(&token));
    assert!(!api_key.contains("k_live_demo"));
}
