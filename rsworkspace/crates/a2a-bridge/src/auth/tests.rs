use std::sync::Arc;

use super::*;
use crate::auth::callout_mint::{BridgeTenantAccount, InProcessCalloutDispatcherMintWire, harness_callout_dispatcher};
use crate::identity::CallerHttpsAuth;

#[tokio::test]
async fn stub_auth_callout_client_returns_not_configured_error() {
    let client = StubAuthCalloutClient;
    let err = client.mint(&CallerHttpsAuth::new("Bearer token")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
    assert!(err.to_string().contains("not configured"));
}

#[tokio::test]
async fn stub_auth_callout_mint_returns_fixture_jwt() {
    let client = StubAuthCalloutMint::fixture().unwrap();
    let jwt = client.mint(&CallerHttpsAuth::new("Bearer ignored")).await.unwrap();
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

#[derive(Clone)]
struct StaticReplyWire(Vec<u8>);

#[async_trait]
impl AuthMintWire for StaticReplyWire {
    async fn roundtrip_message(&self, _: String, _: BytesPayload) -> Result<Vec<u8>, BridgeError> {
        Ok(self.0.clone())
    }
}

#[derive(Clone)]
struct ErrMintWire;

#[async_trait]
impl AuthMintWire for ErrMintWire {
    async fn roundtrip_message(&self, _: String, _: BytesPayload) -> Result<Vec<u8>, BridgeError> {
        Err(BridgeError::Mint("wire roundtrip failed".into()))
    }
}

#[tokio::test]
async fn json_mint_client_new_uses_harness_wire_without_explicit_tenant() {
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(dispatcher, tenant));
    let client = AuthCalloutJsonMintClient::new(wire, "subject");
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .expect("new client mint");
    assert!(jwt.as_str().contains('.'));
}

#[tokio::test]
async fn json_mint_client_accepts_uppercase_bearer_prefix() {
    let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
    let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(dispatcher, tenant.clone()));
    let client = AuthCalloutJsonMintClient::with_tenant_account(wire, "subject", Some(tenant));
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer FIXTURE"))
        .await
        .expect("uppercase bearer");
    assert!(jwt.as_str().contains('.'));
}

#[tokio::test]
async fn json_mint_client_rejects_malformed_response_json() {
    let wire = Arc::new(StaticReplyWire(b"{".to_vec()));
    let client = AuthCalloutJsonMintClient::new(wire, "subject");
    let err = client.mint(&CallerHttpsAuth::new("Bearer x")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Deserialize(_)));
}

#[tokio::test]
async fn json_mint_client_rejects_invalid_user_jwt_in_response() {
    let wire = Arc::new(StaticReplyWire(br#"{"user_jwt":"not-a-jwt"}"#.to_vec()));
    let client = AuthCalloutJsonMintClient::new(wire, "subject");
    let err = client.mint(&CallerHttpsAuth::new("Bearer x")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn json_mint_client_surfaces_wire_roundtrip_error() {
    let client = AuthCalloutJsonMintClient::new(Arc::new(ErrMintWire), "subject");
    let err = client.mint(&CallerHttpsAuth::new("Bearer x")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
    assert!(err.to_string().contains("wire roundtrip failed"));
}
