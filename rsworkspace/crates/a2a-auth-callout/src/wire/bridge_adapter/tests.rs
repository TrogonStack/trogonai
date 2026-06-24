use super::*;
use crate::bridge_mint::{BridgeAuthScheme, BridgeClientInfo, BridgeConnectOpts, BridgeMintRequest};
use crate::error::CredentialError;

#[test]
fn from_bridge_mint_rejects_missing_account() {
    let err = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: None,
        client_info: None,
        connect_opts: None,
    })
    .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest(_))
    ));
}

#[test]
fn from_bridge_mint_rejects_api_key_without_key_material() {
    let err = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: Some("tenant-acme".into()),
        client_info: None,
        connect_opts: Some(BridgeConnectOpts {
            auth_scheme: Some(BridgeAuthScheme::ApiKey),
            api_key: Some("   ".into()),
        }),
    })
    .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest(_))
    ));
}

#[test]
fn from_bridge_mint_rejects_oidc_without_user_jwt() {
    let err = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: Some("tenant-acme".into()),
        client_info: None,
        connect_opts: Some(BridgeConnectOpts {
            auth_scheme: Some(BridgeAuthScheme::Oidc),
            api_key: None,
        }),
    })
    .unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest(_))
    ));
}

#[test]
fn from_bridge_mint_builds_oidc_claims_with_user_jwt() {
    let claims = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: Some("a.b.c".into()),
        account: Some("tenant-acme".into()),
        client_info: None,
        connect_opts: Some(BridgeConnectOpts {
            auth_scheme: Some(BridgeAuthScheme::Oidc),
            api_key: None,
        }),
    })
    .unwrap();
    assert_eq!(claims.requested_account().unwrap().as_str(), "tenant-acme");
    assert_eq!(claims.connect_opts_jwt(), Some("a.b.c"));
    assert!(claims.connect_opts_auth_token().is_none());
}

#[test]
fn from_bridge_mint_builds_mtls_claims_with_client_cert() {
    let claims = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: Some("tenant-acme".into()),
        client_info: Some(BridgeClientInfo {
            client_cert_pem: Some("-----BEGIN CERT-----\n-----END CERT-----".into()),
        }),
        connect_opts: Some(BridgeConnectOpts {
            auth_scheme: Some(BridgeAuthScheme::MTls),
            api_key: None,
        }),
    })
    .unwrap();
    assert_eq!(claims.primary_client_cert().unwrap().as_str(), "-----BEGIN CERT-----\n-----END CERT-----");
}

#[test]
fn from_bridge_mint_builds_api_key_claims() {
    let claims = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: Some("tenant-acme".into()),
        client_info: None,
        connect_opts: Some(BridgeConnectOpts {
            auth_scheme: Some(BridgeAuthScheme::ApiKey),
            api_key: Some("secret-key".into()),
        }),
    })
    .unwrap();
    assert_eq!(claims.requested_account().unwrap().as_str(), "tenant-acme");
    assert_eq!(claims.connect_opts_auth_token(), Some("secret-key"));
    assert!(claims.connect_opts_jwt().is_none());
}
