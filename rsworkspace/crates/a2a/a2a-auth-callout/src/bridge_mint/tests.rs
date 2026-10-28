use super::*;

#[test]
fn bridge_mint_request_debug_redacts_nkey_and_jwt() {
    let request = BridgeMintRequest {
        user_nkey: Some("SUAABBCC".to_string()),
        user_jwt: Some("agent.jwt.value".to_string()),
        account: Some("ACCT".to_string()),
        client_info: None,
        connect_opts: None,
    };
    let debug_output = format!("{request:?}");
    assert!(!debug_output.contains("SUAABBCC"));
    assert!(!debug_output.contains("agent.jwt.value"));
    assert!(debug_output.contains("ACCT"));
    assert!(debug_output.contains("<redacted>"));
}

#[test]
fn bridge_connect_opts_debug_redacts_api_key() {
    let opts = BridgeConnectOpts {
        auth_scheme: Some(BridgeAuthScheme::ApiKey),
        api_key: Some("super-secret-key".to_string()),
    };
    let debug_output = format!("{opts:?}");
    assert!(!debug_output.contains("super-secret-key"));
    assert!(debug_output.contains("<redacted>"));
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
