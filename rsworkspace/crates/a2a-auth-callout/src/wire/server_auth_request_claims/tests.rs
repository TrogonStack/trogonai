use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use nkeys::KeyPair;

use super::*;
use crate::bridge_mint::{BridgeAuthScheme, BridgeClientInfo, BridgeConnectOpts, BridgeMintRequest};
use crate::error::CredentialError;
use crate::wire::test_encode::signed_auth_request;
use crate::wire::{NkeyPublic, ServerAuthRequestEnvelope};

fn decode_request(server: &KeyPair, patch: impl FnMut(&mut Claims<AuthRequest>)) -> ServerAuthRequestClaims {
    let user = KeyPair::new_user();
    let token = signed_auth_request(server, &user, patch);
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, None, None)
        .unwrap()
}

#[test]
fn requested_account_prefers_connect_opts_user() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.user = Some("tenant-from-connect".into());
        c.nats.client_info.user = "tenant-from-client".into();
        c.nats.client_info.name_tag = "tenant-from-tag".into();
    });
    assert_eq!(req.requested_account().unwrap().as_str(), "tenant-from-connect");
}

#[test]
fn requested_account_falls_back_to_client_info_user() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.user = None;
        c.nats.client_info.user = "tenant-from-client".into();
        c.nats.client_info.name_tag = "tenant-from-tag".into();
    });
    assert_eq!(req.requested_account().unwrap().as_str(), "tenant-from-client");
}

#[test]
fn requested_account_falls_back_to_name_tag() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.user = None;
        c.nats.client_info.user = String::new();
        c.nats.client_info.name_tag = "tenant-from-tag".into();
    });
    assert_eq!(req.requested_account().unwrap().as_str(), "tenant-from-tag");
}

#[test]
fn requested_account_rejects_missing_hints() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.user = None;
        c.nats.client_info.user = String::new();
        c.nats.client_info.name_tag = String::new();
    });
    let err = req.requested_account().unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
    ));
}

#[test]
fn connect_opts_auth_token_ignores_blank_values() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.auth_token = Some("   ".into());
    });
    assert!(req.connect_opts_auth_token().is_none());
}

#[test]
fn connect_opts_jwt_reads_from_decoded_claims() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.jwt = Some("opaque.jwt.token".into());
    });
    assert_eq!(req.connect_opts_jwt(), Some("opaque.jwt.token"));
}

#[test]
fn connect_opts_opaque_pass_ignores_empty_values() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |c| {
        c.nats.connect_opts.pass = Some("".into());
    });
    assert!(req.connect_opts_opaque_pass().is_none());
}

#[test]
fn client_tls_pem_certs_reads_from_bridge_mint_payload() {
    let req = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: Some("tenant-acme".into()),
        client_info: Some(BridgeClientInfo {
            client_cert_pem: Some("cert-a".into()),
        }),
        connect_opts: Some(BridgeConnectOpts {
            auth_scheme: Some(BridgeAuthScheme::MTls),
            api_key: None,
        }),
    })
    .unwrap();
    assert_eq!(req.client_tls_pem_certs().len(), 1);
    assert_eq!(req.primary_client_cert().unwrap().as_str(), "cert-a");
}

#[test]
fn debug_includes_user_nkey_and_server_id() {
    let server = KeyPair::new_account();
    let req = decode_request(&server, |_| {});
    let debug = format!("{req:?}");
    assert!(debug.contains("user_nkey"));
    assert!(debug.contains(req.server_id()));
}
