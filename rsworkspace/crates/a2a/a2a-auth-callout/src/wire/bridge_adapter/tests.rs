use super::*;
use crate::bridge_mint::{BridgeAuthScheme, BridgeClientInfo, BridgeConnectOpts, BridgeMintRequest};
use crate::error::CredentialError;

fn mint_request(
    user_jwt: Option<&str>,
    client_cert_pem: Option<&str>,
    auth_scheme: Option<BridgeAuthScheme>,
    api_key: Option<&str>,
) -> BridgeMintRequest {
    BridgeMintRequest {
        user_nkey: None,
        user_jwt: user_jwt.map(str::to_owned),
        account: Some("tenant-acme".into()),
        client_info: client_cert_pem.map(|pem| BridgeClientInfo {
            client_cert_pem: Some(pem.to_owned()),
        }),
        connect_opts: auth_scheme.map(|auth_scheme| BridgeConnectOpts {
            auth_scheme: Some(auth_scheme),
            api_key: api_key.map(str::to_owned),
        }),
    }
}

fn is_invalid_request(err: &AuthCalloutError) -> bool {
    matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest(_))
    )
}

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
    assert!(is_invalid_request(&err));
}

#[test]
fn from_bridge_mint_rejects_api_key_without_key_material() {
    let request = mint_request(None, None, Some(BridgeAuthScheme::ApiKey), Some("   "));
    let err = ServerAuthRequestClaims::from_bridge_mint(request).unwrap_err();
    assert!(is_invalid_request(&err));
}

#[test]
fn from_bridge_mint_rejects_oidc_without_user_jwt() {
    let request = mint_request(None, Some("cert-a"), Some(BridgeAuthScheme::Oidc), None);
    let err = ServerAuthRequestClaims::from_bridge_mint(request).unwrap_err();
    assert!(is_invalid_request(&err));
}

#[test]
fn from_bridge_mint_rejects_mtls_without_a_client_certificate() {
    let request = mint_request(Some("a.b.c"), None, Some(BridgeAuthScheme::MTls), None);
    let err = ServerAuthRequestClaims::from_bridge_mint(request).unwrap_err();
    assert!(is_invalid_request(&err));
}

#[test]
fn from_bridge_mint_declares_the_oidc_user_jwt_as_a_bearer_token() {
    let request = mint_request(Some("a.b.c"), None, Some(BridgeAuthScheme::Oidc), None);
    let claims = ServerAuthRequestClaims::from_bridge_mint(request).unwrap();
    assert_eq!(claims.requested_account().unwrap().as_str(), "tenant-acme");
    assert!(matches!(
        claims.presented_credential(),
        Ok(PresentedCredential::BearerToken(token)) if token.as_str() == "a.b.c"
    ));
}

#[test]
fn from_bridge_mint_without_a_scheme_declares_the_user_jwt_over_the_client_certificate() {
    let request = mint_request(Some("fixture-token"), Some("cert-a"), None, None);
    let claims = ServerAuthRequestClaims::from_bridge_mint(request).unwrap();
    assert!(matches!(
        claims.presented_credential(),
        Ok(PresentedCredential::BearerToken(token)) if token.as_str() == "fixture-token"
    ));
}

#[test]
fn from_bridge_mint_presents_the_mtls_client_certificate() {
    let pem = "-----BEGIN CERT-----\n-----END CERT-----";
    let request = mint_request(Some("ignored"), Some(pem), Some(BridgeAuthScheme::MTls), None);
    let claims = ServerAuthRequestClaims::from_bridge_mint(request).unwrap();
    assert!(matches!(
        claims.presented_credential(),
        Ok(PresentedCredential::ClientCertificate(cert)) if cert.as_str() == pem
    ));
}

#[test]
fn from_bridge_mint_declares_the_api_key() {
    let request = mint_request(None, None, Some(BridgeAuthScheme::ApiKey), Some("secret-key"));
    let claims = ServerAuthRequestClaims::from_bridge_mint(request).unwrap();
    assert_eq!(claims.requested_account().unwrap().as_str(), "tenant-acme");
    assert!(matches!(
        claims.presented_credential(),
        Ok(PresentedCredential::ApiKey(key)) if key.as_str() == "secret-key"
    ));
}

#[test]
fn a_section_that_does_not_read_back_as_its_wire_type_names_itself() {
    let err = decode_section::<ClientTLS>("client_tls", serde_json::Value::String("not a section".into())).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::BridgeWireFormat {
            section: "client_tls",
            ..
        }
    ));
}
