use std::sync::Arc;

use a2a_auth_callout::caller_id_from_minted_jwt;
use a2a_auth_callout::{BridgeConnectOpts, BridgeMintRequest};

use super::*;
use crate::auth::BytesPayload;
use crate::auth::{AuthCalloutClient, AuthCalloutJsonMintClient};
use crate::identity::CallerHttpsAuth;

#[tokio::test]
async fn harness_mint_wire_returns_deterministic_caller_id_jwt() {
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(dispatcher, tenant.clone()));
    let client = AuthCalloutJsonMintClient::with_tenant_account(wire, "a2a.bridge.auth.callout.request", Some(tenant));
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .expect("harness mint");
    let caller = caller_id_from_minted_jwt(jwt.as_str()).expect("caller_id claim");
    assert_eq!(caller.as_str(), "bridge-harness-caller");
}

#[test]
fn bridge_tenant_account_rejects_empty() {
    let err = BridgeTenantAccount::new("   ").unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn mint_wire_rejects_malformed_json() {
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant);

    let err = wire
        .roundtrip_message("subject".into(), BytesPayload(vec![b'{']))
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Deserialize(_)));
}

#[tokio::test]
async fn mint_wire_increments_mint_count() {
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(dispatcher, tenant.clone()));
    let client =
        AuthCalloutJsonMintClient::with_tenant_account(wire.clone(), "a2a.bridge.auth.callout.request", Some(tenant));

    client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .expect("harness mint");

    assert_eq!(wire.mint_count(), 1);
}

#[tokio::test]
async fn mint_wire_fails_when_from_bridge_mint_errors_on_blank_account() {
    // Pass a BridgeMintRequest with account = Some("") (blank, not None).
    // The wire only replaces account when it is None, so the blank account
    // reaches from_bridge_mint which rejects it via RequestedAccount::new("").
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant);

    let request = BridgeMintRequest {
        user_nkey: None,
        user_jwt: Some("token".into()),
        account: Some("   ".into()), // blank but not None → not replaced by tenant
        client_info: None,
        connect_opts: None,
    };
    let payload = serde_json::to_vec(&request).unwrap();
    let err = wire
        .roundtrip_message("subject".into(), BytesPayload(payload))
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn mint_wire_fails_when_dispatcher_rejects_unknown_account() {
    // The harness dispatcher only resolves "tenant-harness"; using a different
    // account causes dispatcher.dispatch to return an error.
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
    let wire = InProcessCalloutDispatcherMintWire::new(dispatcher, tenant);

    let request = BridgeMintRequest {
        user_nkey: None,
        user_jwt: Some("token".into()),
        account: Some("unknown-account".into()),
        client_info: None,
        connect_opts: Some(BridgeConnectOpts {
            auth_scheme: Some(a2a_auth_callout::BridgeAuthScheme::Oidc),
            api_key: None,
        }),
    };
    let payload = serde_json::to_vec(&request).unwrap();
    let err = wire
        .roundtrip_message("subject".into(), BytesPayload(payload))
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}
