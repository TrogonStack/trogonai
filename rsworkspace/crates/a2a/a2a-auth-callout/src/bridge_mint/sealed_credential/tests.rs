use nkeys::XKey;

use super::*;

fn callout_xkey() -> CalloutXkey {
    CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap()
}

fn bearer(token: &str) -> BridgeCredential {
    BridgeCredential::BearerToken(BearerToken::new(token))
}

fn sealed_document(plaintext: &[u8], callout: &CalloutXkey) -> SealedBridgeCredential {
    let sender = XKey::new();
    let sealed = sender
        .seal(plaintext, &callout.public_key().to_xkey().unwrap())
        .unwrap();
    serde_json::from_value(serde_json::json!({
        "sender": sender.public_key(),
        "sealed": URL_SAFE_NO_PAD.encode(sealed),
    }))
    .unwrap()
}

#[test]
fn a_sealed_bearer_token_opens_with_the_callout_xkey() {
    let callout = callout_xkey();
    let sealed = SealedBridgeCredential::seal(&bearer("a.b.c"), &callout.public_key()).unwrap();
    assert_eq!(sealed.open(&callout).unwrap(), bearer("a.b.c"));
}

#[test]
fn a_sealed_api_key_opens_with_the_callout_xkey() {
    let callout = callout_xkey();
    let key = BridgeCredential::ApiKey(ApiKey::new("k_live_demo").unwrap());
    let sealed = SealedBridgeCredential::seal(&key, &callout.public_key()).unwrap();
    assert_eq!(sealed.open(&callout).unwrap(), key);
}

#[test]
fn a_sealed_credential_survives_its_wire_document() {
    let callout = callout_xkey();
    let sealed = SealedBridgeCredential::seal(&bearer("a.b.c"), &callout.public_key()).unwrap();
    let decoded: SealedBridgeCredential = serde_json::from_slice(&serde_json::to_vec(&sealed).unwrap()).unwrap();
    assert_eq!(decoded.open(&callout).unwrap(), bearer("a.b.c"));
}

#[test]
fn the_sealed_document_does_not_carry_the_secret() {
    let callout = callout_xkey();
    let sealed = SealedBridgeCredential::seal(&bearer("caller-secret-token"), &callout.public_key()).unwrap();
    assert!(!serde_json::to_string(&sealed).unwrap().contains("caller-secret-token"));
    assert!(!format!("{sealed:?}").contains("caller-secret-token"));
}

#[test]
fn each_seal_uses_a_fresh_sender_key() {
    let callout = callout_xkey();
    let first = SealedBridgeCredential::seal(&bearer("a.b.c"), &callout.public_key()).unwrap();
    let second = SealedBridgeCredential::seal(&bearer("a.b.c"), &callout.public_key()).unwrap();
    assert_ne!(first.sender, second.sender);
    assert_ne!(first.sender, callout.public_key());
}

#[test]
fn a_blank_secret_is_not_sealed() {
    let err = SealedBridgeCredential::seal(&bearer("   "), &callout_xkey().public_key()).unwrap_err();
    assert!(matches!(err, BridgeCredentialError::BlankSecret));
}

#[test]
fn a_credential_sealed_to_another_callout_does_not_open() {
    let sealed = SealedBridgeCredential::seal(&bearer("a.b.c"), &callout_xkey().public_key()).unwrap();
    let err = sealed.open(&callout_xkey()).unwrap_err();
    assert!(matches!(
        err,
        BridgeCredentialError::CannotOpen(XkeyExchangeError::CannotOpen)
    ));
}

#[test]
fn a_sealed_document_that_is_not_a_credential_is_malformed() {
    let callout = callout_xkey();
    let plaintexts: [&[u8]; 3] = [
        b"not json",
        br#"{"scheme":"password","secret":"x"}"#,
        br#"{"scheme":"bearer_token","token":"a.b.c","account":"tenant"}"#,
    ];
    for plaintext in plaintexts {
        let err = sealed_document(plaintext, &callout).open(&callout).unwrap_err();
        assert!(matches!(err, BridgeCredentialError::Malformed(_)));
    }
}

#[test]
fn a_sealed_blank_secret_is_refused_on_open() {
    let callout = callout_xkey();
    let plaintexts: [&[u8]; 2] = [
        br#"{"scheme":"bearer_token","token":"  "}"#,
        br#"{"scheme":"api_key","key":""}"#,
    ];
    for plaintext in plaintexts {
        let err = sealed_document(plaintext, &callout).open(&callout).unwrap_err();
        assert!(matches!(err, BridgeCredentialError::BlankSecret));
    }
}

#[test]
fn a_sealed_credential_with_a_plaintext_field_does_not_decode() {
    let callout = callout_xkey();
    let sealed = SealedBridgeCredential::seal(&bearer("a.b.c"), &callout.public_key()).unwrap();
    let mut document = serde_json::to_value(&sealed).unwrap();
    document["token"] = serde_json::Value::String("a.b.c".into());
    assert!(serde_json::from_value::<SealedBridgeCredential>(document).is_err());
}

#[test]
fn a_sealed_credential_from_a_sender_that_is_not_a_curve_key_does_not_decode() {
    let document = serde_json::json!({"sender": "not-an-xkey", "sealed": ""});
    assert!(serde_json::from_value::<SealedBridgeCredential>(document).is_err());
}

#[test]
fn an_opened_credential_is_presented_under_its_own_scheme() {
    assert!(matches!(
        PresentedCredential::from(bearer("a.b.c")),
        PresentedCredential::BearerToken(token) if token.as_str() == "a.b.c"
    ));
    assert!(matches!(
        PresentedCredential::from(BridgeCredential::ApiKey(ApiKey::new("k_live_demo").unwrap())),
        PresentedCredential::ApiKey(key) if key.as_str() == "k_live_demo"
    ));
}
