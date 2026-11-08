use std::sync::Arc;

use a2a_auth_callout::{
    BridgeCredential, BridgeCredentialError, SealedBridgeCredential, caller_id_from_minted_jwt,
    subject_principal_from_minted_jwt,
};

use super::*;
use crate::auth::BytesPayload;
use crate::auth::{AuthCalloutClient, AuthCalloutJsonMintClient};
use crate::identity::CallerHttpsAuth;

const MINT_SUBJECT: &str = "a2a.bridge.auth.callout.request";

fn sealed_fixture_token(callout: &CalloutXkey) -> SealedBridgeCredential {
    SealedBridgeCredential::seal(
        &BridgeCredential::BearerToken(BearerToken::new("fixture-token")),
        &callout.public_key(),
    )
    .unwrap()
}

async fn roundtrip(wire: &InProcessCalloutDispatcherMintWire, payload: Vec<u8>) -> Result<Vec<u8>, BridgeError> {
    wire.roundtrip_message("subject".into(), BytesPayload(payload)).await
}

#[tokio::test]
async fn harness_mint_wire_returns_deterministic_caller_id_jwt() {
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let callout = harness_callout_xkey();
    let client_xkey = callout.public_key();
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(
        dispatcher,
        tenant.clone(),
        Some(callout),
    ));
    let client =
        AuthCalloutJsonMintClient::with_tenant_account(wire, MINT_SUBJECT, Some(tenant)).sealing_to(client_xkey);
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .expect("harness mint");
    let caller = caller_id_from_minted_jwt(jwt.as_str()).expect("caller_id claim");
    assert_eq!(caller.as_str(), "bridge-harness-caller");
    assert_eq!(
        subject_principal_from_minted_jwt(jwt.as_str()).expect("principal claims"),
        trogon_identity_types::SubjectPrincipal::from_tokens("person", "bridge-harness-caller").expect("principal")
    );
}

#[test]
fn bridge_tenant_account_rejects_empty() {
    let err = BridgeTenantAccount::new("   ").unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn mint_wire_rejects_malformed_json() {
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant, None);
    let err = roundtrip(&wire, vec![b'{']).await.unwrap_err();
    assert!(matches!(err, BridgeError::Deserialize(_)));
}

#[tokio::test]
async fn mint_wire_refuses_a_request_carrying_a_plaintext_credential() {
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant, Some(harness_callout_xkey()));
    let payload = br#"{"account":"tenant","user_jwt":"fixture-token"}"#.to_vec();
    let err = roundtrip(&wire, payload).await.unwrap_err();
    assert!(matches!(err, BridgeError::Deserialize(_)));
}

#[tokio::test]
async fn mint_wire_without_a_callout_xkey_refuses_a_sealed_credential() {
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant, None);
    let request = BridgeMintRequest {
        sealed_credential: Some(sealed_fixture_token(&harness_callout_xkey())),
        ..BridgeMintRequest::default()
    };
    let err = roundtrip(&wire, serde_json::to_vec(&request).unwrap())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        BridgeError::MintCredential(BridgeCredentialError::NoCalloutXkey)
    ));
}

#[tokio::test]
async fn mint_wire_increments_mint_count() {
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let callout = harness_callout_xkey();
    let client_xkey = callout.public_key();
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(
        dispatcher,
        tenant.clone(),
        Some(callout),
    ));
    let client = AuthCalloutJsonMintClient::with_tenant_account(wire.clone(), MINT_SUBJECT, Some(tenant))
        .sealing_to(client_xkey);

    client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .expect("harness mint");

    assert_eq!(wire.mint_count(), 1);
}

#[tokio::test]
async fn mint_wire_fails_when_from_bridge_mint_errors_on_blank_account() {
    // The wire only fills in the tenant when the account is absent, so a blank
    // account reaches from_bridge_mint, which refuses it.
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let callout = harness_callout_xkey();
    let request = BridgeMintRequest {
        account: Some("   ".into()),
        sealed_credential: Some(sealed_fixture_token(&callout)),
        ..BridgeMintRequest::default()
    };
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant, Some(callout));
    let err = roundtrip(&wire, serde_json::to_vec(&request).unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn mint_wire_fails_when_dispatcher_rejects_unknown_account() {
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let callout = harness_callout_xkey();
    let request = BridgeMintRequest {
        account: Some("unknown-account".into()),
        sealed_credential: Some(sealed_fixture_token(&callout)),
        ..BridgeMintRequest::default()
    };
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant, Some(callout));
    let err = roundtrip(&wire, serde_json::to_vec(&request).unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}
