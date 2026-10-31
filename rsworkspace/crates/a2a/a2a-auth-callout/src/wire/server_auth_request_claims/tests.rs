use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use nkeys::KeyPair;

use super::*;
use crate::bridge_mint::{BridgeClientInfo, BridgeMintRequest};
use crate::error::CredentialError;
use crate::wire::test_encode::{FixtureServer, verified_client_tls};

fn decode_request(server: &FixtureServer, patch: impl FnMut(&mut Claims<AuthRequest>)) -> ServerAuthRequestClaims {
    server.request(&KeyPair::new_user(), patch)
}

fn presents_api_key(request: &ServerAuthRequestClaims, expected: &str) -> bool {
    matches!(request.presented_credential(), Ok(PresentedCredential::ApiKey(key)) if key.as_str() == expected)
}

#[test]
fn requested_account_reads_connect_opts_user() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.user = Some("tenant-from-connect".into());
        c.nats.client_info.user = "sentinel-user".into();
        c.nats.client_info.name_tag = "sentinel-name".into();
    });
    assert_eq!(req.requested_account().unwrap().as_str(), "tenant-from-connect");
}

#[test]
fn requested_account_does_not_take_the_sentinel_client_info_for_a_tenant() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.user = None;
        c.nats.client_info.user = "tenant-from-client".into();
        c.nats.client_info.name_tag = "tenant-from-tag".into();
    });
    let err = req.requested_account().unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[test]
fn requested_account_rejects_a_blank_user() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.user = Some("   ".into());
    });
    let err = req.requested_account().unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[test]
fn the_sentinel_jwt_alone_presents_no_credential() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.jwt = Some("sentinel.user.jwt".into());
        c.nats.connect_opts.auth_token = None;
        c.nats.connect_opts.pass = None;
    });
    let err = req.presented_credential().unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest(_))
    ));
}

#[test]
fn auth_token_is_read_before_pass() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.auth_token = Some("k_from_token".into());
        c.nats.connect_opts.pass = Some("k_from_pass".into());
    });
    assert!(presents_api_key(&req, "k_from_token"));
}

#[test]
fn pass_is_read_when_auth_token_is_absent() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.auth_token = None;
        c.nats.connect_opts.pass = Some("k_from_pass".into());
    });
    assert!(presents_api_key(&req, "k_from_pass"));
}

#[test]
fn a_blank_secret_is_not_presented() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.auth_token = Some("   ".into());
        c.nats.connect_opts.pass = Some("k_from_pass".into());
    });
    assert!(presents_api_key(&req, "k_from_pass"));
}

#[test]
fn a_bearer_token_in_auth_token_is_presented_as_one() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |_| {});
    assert!(matches!(
        req.presented_credential(),
        Ok(PresentedCredential::BearerToken(_))
    ));
}

#[test]
fn a_secret_is_presented_instead_of_the_client_certificate() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.auth_token = Some("k_from_token".into());
        c.nats.client_tls = Some(verified_client_tls(&["cert-a"]));
    });
    assert!(presents_api_key(&req, "k_from_token"));
}

#[test]
fn the_client_certificate_is_presented_when_no_secret_is() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.auth_token = None;
        c.nats.connect_opts.pass = None;
        c.nats.client_tls = Some(verified_client_tls(&["cert-a", "issuer-b"]));
    });
    assert!(matches!(
        req.presented_credential(),
        Ok(PresentedCredential::ClientCertificate(pem)) if pem.as_str() == "cert-a"
    ));
}

#[test]
fn client_tls_pem_certs_reads_from_bridge_mint_payload() {
    let req = ServerAuthRequestClaims::from_bridge_mint(
        BridgeMintRequest {
            account: Some("tenant-acme".into()),
            client_info: Some(BridgeClientInfo {
                client_cert_pem: Some("cert-a".into()),
            }),
            ..BridgeMintRequest::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(req.client_tls_pem_certs().len(), 1);
    assert_eq!(req.primary_client_cert().unwrap().as_str(), "cert-a");
    assert!(matches!(
        req.presented_credential(),
        Ok(PresentedCredential::ClientCertificate(pem)) if pem.as_str() == "cert-a"
    ));
}

#[test]
fn debug_includes_user_nkey_and_server_id() {
    let server = FixtureServer::generate();
    let req = decode_request(&server, |_| {});
    let debug = format!("{req:?}");
    assert!(debug.contains("user_nkey"));
    assert!(debug.contains(req.server_id()));
}

#[test]
fn client_tls_pem_certs_reads_verified_chains_from_nats_json() {
    let server = FixtureServer::generate();
    let nats_json = serde_json::json!({
        "client_tls": {
            "verified_chains": [["chain-cert-1", "chain-cert-2"]]
        }
    });
    let req = ServerAuthRequestClaims::from_decoded(server.claims(&KeyPair::new_user(), |_| {}), nats_json);
    let certs = req.client_tls_pem_certs();
    assert_eq!(certs.len(), 2);
    assert_eq!(certs[0].as_str(), "chain-cert-1");
    assert_eq!(certs[1].as_str(), "chain-cert-2");
}
