use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use nkeys::KeyPair;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair as CertificateKeyPair};
use time::OffsetDateTime;

use super::*;
use crate::bridge_mint::{BridgeClientInfo, BridgeMintRequest};
use crate::credentials::mtls::{TrustAnchorPem, X509MtlsVerifier};
use crate::error::CredentialError;
use crate::jwt::AudienceAccount;
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
        Ok(PresentedCredential::ClientCertificate(pem)) if pem.as_str() == "cert-a\nissuer-b"
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

#[test]
fn a_request_from_nats_server_admits_the_callers_own_connection() {
    let server = FixtureServer::generate();
    assert_eq!(
        decode_request(&server, |_| {}).admission_route(),
        AdmissionRoute::Direct
    );
}

fn request_with_tls(certs: &[String], verified_chains: &[Vec<String>]) -> ServerAuthRequestClaims {
    FixtureServer::generate().request(&KeyPair::new_user(), |claims| {
        claims.nats.connect_opts.auth_token = None;
        claims.nats.connect_opts.pass = None;
        claims.nats.client_tls = Some(
            serde_json::from_value(serde_json::json!({
                "version": "1.3",
                "cipher": "TLS_AES_128_GCM_SHA256",
                "certs": certs,
                "verified_chains": verified_chains,
            }))
            .unwrap(),
        );
    })
}

#[test]
fn tls_metadata_without_a_selected_chain_cannot_supply_a_credential() {
    let request = request_with_tls(&[], &[]);
    assert!(request.client_tls_pem_certs().is_empty());
    assert!(matches!(
        request.presented_credential(),
        Err(AuthCalloutError::CredentialVerification(
            CredentialError::InvalidRequest(_)
        ))
    ));
}

#[test]
fn an_empty_selected_verified_chain_cannot_borrow_a_later_leaf() {
    let (leaf, issuer, _) = client_chain();
    let request = request_with_tls(&[], &[vec![], vec![leaf, issuer]]);
    assert!(request.primary_client_cert().is_none());
    assert!(matches!(
        request.presented_credential(),
        Err(AuthCalloutError::CredentialVerification(
            CredentialError::InvalidRequest(_)
        ))
    ));
}

fn client_chain() -> (String, String, String) {
    let root_key = CertificateKeyPair::generate().unwrap();
    let mut root_params = CertificateParams::default();
    root_params.distinguished_name.push(DnType::CommonName, "root");
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let root = root_params.self_signed(&root_key).unwrap();
    let issuer_key = CertificateKeyPair::generate().unwrap();
    let mut issuer_params = CertificateParams::default();
    issuer_params
        .distinguished_name
        .push(DnType::CommonName, "intermediate");
    issuer_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    let issuer = issuer_params
        .signed_by(&issuer_key, &Issuer::from_params(&root_params, &root_key))
        .unwrap();
    let leaf_key = CertificateKeyPair::generate().unwrap();
    let mut leaf_params = CertificateParams::default();
    leaf_params.distinguished_name.push(DnType::CommonName, "client");
    let leaf = leaf_params
        .signed_by(&leaf_key, &Issuer::from_params(&issuer_params, &issuer_key))
        .unwrap();
    (leaf.pem(), issuer.pem(), root.pem())
}

fn assert_request_certificate_verifies(request: &ServerAuthRequestClaims, root: &str) {
    let chain = request.primary_client_cert().expect("presented certificate chain");
    let claims = X509MtlsVerifier::new(TrustAnchorPem::new(root))
        .verify_sync(&chain, &AudienceAccount::new("tenant"), OffsetDateTime::now_utc())
        .expect("the request must preserve the intermediate for verification");
    assert!(claims.claims().sub.as_str().contains("client"));
}

#[test]
fn presented_chain_survives_signed_request_and_verifier_roundtrip() {
    let (leaf, issuer, root) = client_chain();
    let request = request_with_tls(&[leaf, issuer], &[]);
    assert_request_certificate_verifies(&request, &root);
}

#[test]
fn verified_chain_survives_signed_request_and_verifier_roundtrip() {
    let (leaf, issuer, root) = client_chain();
    let request = request_with_tls(&[], &[vec![leaf, issuer]]);
    assert_request_certificate_verifies(&request, &root);
}

#[test]
fn alternate_verified_chains_are_not_flattened_together() {
    let request = request_with_tls(
        &[],
        &[
            vec!["same-leaf".into(), "first-issuer".into()],
            vec!["same-leaf".into(), "alternate-issuer".into()],
        ],
    );
    let chain = request.client_tls_pem_certs();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].as_str(), "same-leaf");
    assert_eq!(chain[1].as_str(), "first-issuer");
}

#[test]
fn conflicting_verified_leaf_identities_are_refused() {
    let request = request_with_tls(
        &[],
        &[
            vec!["first-leaf".into(), "first-issuer".into()],
            vec!["other-leaf".into(), "other-issuer".into()],
        ],
    );
    assert!(request.presented_credential().is_err());
}

#[test]
fn conflicting_presented_and_verified_leaf_identities_are_refused() {
    let request = request_with_tls(&["presented-leaf".into()], &[vec!["different-verified-leaf".into()]]);
    assert!(request.presented_credential().is_err());
}

#[test]
fn a_presented_chain_never_borrows_verified_intermediates() {
    let request = request_with_tls(
        &["same-leaf".into(), "presented-issuer".into()],
        &[vec!["same-leaf".into(), "verified-issuer".into()]],
    );
    assert_eq!(
        request.primary_client_cert().unwrap().as_str(),
        "same-leaf\npresented-issuer"
    );
}

#[test]
fn the_same_leaf_with_different_pem_whitespace_is_not_a_conflict() {
    let (leaf, issuer, root) = client_chain();
    let presented_leaf = leaf.trim().to_owned();
    let request = request_with_tls(&[presented_leaf, issuer.clone()], &[vec![leaf, issuer]]);
    assert_request_certificate_verifies(&request, &root);
}

#[test]
fn a_legacy_single_pem_chain_keeps_its_exact_shape() {
    let (leaf, issuer, root) = client_chain();
    let pem = leaf + &issuer;
    let request = request_with_tls(std::slice::from_ref(&pem), &[]);
    assert_eq!(request.primary_client_cert().unwrap().as_str(), pem);
    assert_request_certificate_verifies(&request, &root);
}
