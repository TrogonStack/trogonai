use nkeys::XKey;

use super::*;
use crate::admission_keys::CalloutXkey;
use crate::credentials::oidc::BearerToken;

#[test]
fn bridge_mint_request_debug_redacts_the_nkey_and_the_sealed_credential() {
    let callout = CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap();
    let credential = BridgeCredential::BearerToken(BearerToken::new("agent.jwt.value"));
    let request = BridgeMintRequest {
        user_nkey: Some("SUAABBCC".to_string()),
        account: Some("ACCT".to_string()),
        sealed_credential: Some(SealedBridgeCredential::seal(&credential, &callout.public_key()).unwrap()),
        ..BridgeMintRequest::default()
    };
    let debug_output = format!("{request:?}");
    assert!(!debug_output.contains("SUAABBCC"));
    assert!(!debug_output.contains("agent.jwt.value"));
    assert!(debug_output.contains("ACCT"));
    assert!(debug_output.contains("<redacted>"));
}

#[test]
fn a_bridge_mint_request_naming_a_plaintext_credential_does_not_decode() {
    let documents = [
        r#"{"account":"ACCT","user_jwt":"agent.jwt.value"}"#,
        r#"{"account":"ACCT","connect_opts":{"auth_scheme":"api_key","api_key":"super-secret-key"}}"#,
    ];
    for document in documents {
        assert!(serde_json::from_str::<BridgeMintRequest>(document).is_err());
    }
}

#[test]
fn bridge_mint_response_debug_redacts_user_jwt() {
    let response = BridgeMintResponse {
        user_jwt: "issued.jwt.value".to_string(),
    };
    let debug_output = format!("{response:?}");
    assert!(!debug_output.contains("issued.jwt.value"));
    assert!(debug_output.contains("<redacted>"));
}
