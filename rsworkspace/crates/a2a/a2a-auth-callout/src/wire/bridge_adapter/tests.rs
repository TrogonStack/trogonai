use nkeys::XKey;

use super::*;
use crate::bridge_mint::{BridgeClientInfo, BridgeCredential, SealedBridgeCredential};
use crate::credentials::api_key::ApiKey;
use crate::credentials::oidc::BearerToken;
use crate::permissions::AdmissionRoute;

const CLIENT_CERT_PEM: &str = "-----BEGIN CERT-----\n-----END CERT-----";

fn callout_xkey() -> CalloutXkey {
    CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap()
}

fn bearer(token: &str) -> BridgeCredential {
    BridgeCredential::BearerToken(BearerToken::new(token))
}

fn mint_request(sealed_credential: Option<SealedBridgeCredential>, client_cert_pem: Option<&str>) -> BridgeMintRequest {
    BridgeMintRequest {
        account: Some("tenant-acme".into()),
        client_info: client_cert_pem.map(|pem| BridgeClientInfo {
            client_cert_pem: Some(pem.to_owned()),
        }),
        sealed_credential,
        ..BridgeMintRequest::default()
    }
}

fn sealed_to(credential: &BridgeCredential, callout: &CalloutXkey) -> Option<SealedBridgeCredential> {
    Some(SealedBridgeCredential::seal(credential, &callout.public_key()).unwrap())
}

fn is_invalid_request(err: &AuthCalloutError) -> bool {
    matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest(_))
    )
}

#[test]
fn from_bridge_mint_rejects_missing_account() {
    let err = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest::default(), None).unwrap_err();
    assert!(is_invalid_request(&err));
}

#[test]
fn from_bridge_mint_declares_a_sealed_bearer_token() {
    let callout = callout_xkey();
    let request = mint_request(sealed_to(&bearer("a.b.c"), &callout), None);
    let claims = ServerAuthRequestClaims::from_bridge_mint(request, Some(&callout)).unwrap();
    assert_eq!(claims.requested_account().unwrap().as_str(), "tenant-acme");
    assert!(matches!(
        claims.presented_credential(),
        Ok(PresentedCredential::BearerToken(token)) if token.as_str() == "a.b.c"
    ));
}

#[test]
fn from_bridge_mint_declares_a_sealed_api_key() {
    let callout = callout_xkey();
    let key = BridgeCredential::ApiKey(ApiKey::new("secret-key").unwrap());
    let claims =
        ServerAuthRequestClaims::from_bridge_mint(mint_request(sealed_to(&key, &callout), None), Some(&callout))
            .unwrap();
    assert!(matches!(
        claims.presented_credential(),
        Ok(PresentedCredential::ApiKey(key)) if key.as_str() == "secret-key"
    ));
}

#[test]
fn from_bridge_mint_prefers_the_sealed_credential_over_the_client_certificate() {
    let callout = callout_xkey();
    let request = mint_request(sealed_to(&bearer("a.b.c"), &callout), Some(CLIENT_CERT_PEM));
    let claims = ServerAuthRequestClaims::from_bridge_mint(request, Some(&callout)).unwrap();
    assert!(matches!(
        claims.presented_credential(),
        Ok(PresentedCredential::BearerToken(token)) if token.as_str() == "a.b.c"
    ));
}

#[test]
fn a_callout_without_an_xkey_refuses_a_sealed_credential() {
    let request = mint_request(sealed_to(&bearer("a.b.c"), &callout_xkey()), None);
    let err = ServerAuthRequestClaims::from_bridge_mint(request, None).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::BridgeCredential(BridgeCredentialError::NoCalloutXkey)
    ));
}

#[test]
fn a_credential_sealed_to_another_callout_is_refused() {
    let request = mint_request(sealed_to(&bearer("a.b.c"), &callout_xkey()), None);
    let err = ServerAuthRequestClaims::from_bridge_mint(request, Some(&callout_xkey())).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::BridgeCredential(BridgeCredentialError::CannotOpen(_))
    ));
}

#[test]
fn from_bridge_mint_presents_the_client_certificate_when_nothing_is_sealed() {
    for callout in [None, Some(callout_xkey())] {
        let claims =
            ServerAuthRequestClaims::from_bridge_mint(mint_request(None, Some(CLIENT_CERT_PEM)), callout.as_ref())
                .unwrap();
        assert!(matches!(
            claims.presented_credential(),
            Ok(PresentedCredential::ClientCertificate(cert)) if cert.as_str() == CLIENT_CERT_PEM
        ));
    }
}

#[test]
fn a_bridge_request_presenting_nothing_presents_no_credential() {
    let callout = callout_xkey();
    let claims = ServerAuthRequestClaims::from_bridge_mint(mint_request(None, None), Some(&callout)).unwrap();
    assert!(is_invalid_request(&claims.presented_credential().unwrap_err()));
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

#[test]
fn a_bridge_mint_request_admits_a_bridge_routed_caller() {
    let claims = ServerAuthRequestClaims::from_bridge_mint(mint_request(None, Some(CLIENT_CERT_PEM)), None).unwrap();
    assert_eq!(claims.admission_route(), AdmissionRoute::BridgeRouted);
}
