use std::sync::Arc;

use super::*;
use crate::auth::callout_mint::{BridgeTenantAccount, InProcessCalloutDispatcherMintWire, harness_callout_dispatcher};
use crate::identity::CallerHttpsAuth;

#[tokio::test]
async fn stub_auth_callout_client_returns_not_configured_error() {
    let client = StubAuthCalloutClient;
    let err = client
        .mint(&CallerHttpsAuth::new("Bearer token"))
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
    assert!(err.to_string().contains("not configured"));
}

#[tokio::test]
async fn stub_auth_callout_mint_returns_fixture_jwt() {
    let client = StubAuthCalloutMint::fixture().unwrap();
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer ignored"))
        .await
        .unwrap();
    assert_eq!(jwt.as_str(), "eyJhbGciOiJub25lIn0.eyJzdWIiOiJoYXJuZXNzIn0.sig");
}

#[test]
fn stub_auth_callout_mint_rejects_invalid_jwt_shape() {
    let err = StubAuthCalloutMint::new("not-a-jwt").unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[test]
fn default_mint_subject_is_stable() {
    struct DummyWire;
    #[async_trait]
    impl AuthMintWire for DummyWire {
        async fn roundtrip_message(&self, _: String, _: BytesPayload) -> Result<Vec<u8>, BridgeError> {
            Ok(vec![])
        }
    }
    assert_eq!(
        AuthCalloutJsonMintClient::<DummyWire>::default_mint_subject(),
        "a2a.bridge.auth.callout.request"
    );
}

#[tokio::test]
async fn json_mint_client_accepts_lowercase_bearer_prefix() {
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("lowercase-caller"));
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(dispatcher, tenant.clone()));
    let client = AuthCalloutJsonMintClient::with_tenant_account(wire, "subject", Some(tenant));
    let jwt = client
        .mint(&CallerHttpsAuth::new("bearer fixture-token"))
        .await
        .expect("lowercase bearer");
    assert!(jwt.as_str().contains('.'));
}

#[test]
fn bytes_payload_wraps_vec() {
    let payload = BytesPayload(vec![1, 2, 3]);
    assert_eq!(payload.0, vec![1, 2, 3]);
}
