use std::sync::{Arc, Mutex};

use super::*;
use crate::auth::callout_mint::{InProcessCalloutDispatcherMintWire, harness_callout_dispatcher, harness_callout_xkey};
use crate::identity::CallerHttpsAuth;

fn harness_client(caller_id: &str) -> AuthCalloutJsonMintClient<InProcessCalloutDispatcherMintWire> {
    let (dispatcher, tenant) = harness_callout_dispatcher(caller_id);
    let callout = harness_callout_xkey();
    let client_xkey = callout.public_key();
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(
        dispatcher,
        tenant.clone(),
        Some(callout),
    ));
    AuthCalloutJsonMintClient::with_tenant_account(wire, "subject", Some(tenant)).sealing_to(client_xkey)
}

fn sealing_client<W: AuthMintWire + 'static>(wire: Arc<W>) -> AuthCalloutJsonMintClient<W> {
    AuthCalloutJsonMintClient::new(wire, "subject").sealing_to(harness_callout_xkey().public_key())
}

#[tokio::test]
async fn stub_auth_callout_client_returns_not_configured_error() {
    let client = StubAuthCalloutClient;
    let err = client.mint(&CallerHttpsAuth::new("Bearer token")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn stub_auth_callout_mint_returns_fixture_jwt() {
    let client = StubAuthCalloutMint::fixture().unwrap();
    let jwt = client.mint(&CallerHttpsAuth::new("Bearer ignored")).await.unwrap();
    assert_eq!(
        jwt.as_str(),
        "eyJhbGciOiJub25lIn0.eyJzdWIiOiJoYXJuZXNzIiwiY2FsbGVyX2lkIjoiaGFybmVzcyIsInByaW5jaXBhbF9raW5kIjoicGVyc29uIn0.sig"
    );
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
    let client = harness_client("lowercase-caller");
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

#[derive(Default)]
struct CapturingWire {
    payloads: Mutex<Vec<Vec<u8>>>,
}

impl CapturingWire {
    fn payloads(&self) -> Vec<Vec<u8>> {
        self.payloads.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuthMintWire for CapturingWire {
    async fn roundtrip_message(&self, _: String, payload: BytesPayload) -> Result<Vec<u8>, BridgeError> {
        self.payloads.lock().unwrap().push(payload.0);
        Ok(br#"{"user_jwt":"header.payload.signature"}"#.to_vec())
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
    let (dispatcher, tenant) = harness_callout_dispatcher("bridge-harness-caller");
    let callout = harness_callout_xkey();
    let client_xkey = callout.public_key();
    let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(
        dispatcher,
        tenant,
        Some(callout),
    ));
    let client = AuthCalloutJsonMintClient::new(wire, "subject").sealing_to(client_xkey);
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .expect("new client mint");
    assert!(jwt.as_str().contains('.'));
}

#[tokio::test]
async fn json_mint_client_accepts_uppercase_bearer_prefix() {
    let client = harness_client("bridge-harness-caller");
    let jwt = client
        .mint(&CallerHttpsAuth::new("Bearer FIXTURE"))
        .await
        .expect("uppercase bearer");
    assert!(jwt.as_str().contains('.'));
}

#[tokio::test]
async fn json_mint_client_rejects_malformed_response_json() {
    let client = sealing_client(Arc::new(StaticReplyWire(b"{".to_vec())));
    let err = client.mint(&CallerHttpsAuth::new("Bearer x")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Deserialize(_)));
}

#[tokio::test]
async fn json_mint_client_rejects_invalid_user_jwt_in_response() {
    let client = sealing_client(Arc::new(StaticReplyWire(br#"{"user_jwt":"not-a-jwt"}"#.to_vec())));
    let err = client.mint(&CallerHttpsAuth::new("Bearer x")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn json_mint_client_surfaces_wire_roundtrip_error() {
    let client = sealing_client(Arc::new(ErrMintWire));
    let err = client.mint(&CallerHttpsAuth::new("Bearer x")).await.unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
}

#[tokio::test]
async fn json_mint_client_without_a_callout_xkey_refuses_before_the_exchange() {
    let wire = Arc::new(CapturingWire::default());
    let client = AuthCalloutJsonMintClient::new(wire.clone(), "subject");
    let err = client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        BridgeError::MintCredential(BridgeCredentialError::NoCalloutXkey)
    ));
    assert!(wire.payloads().is_empty());
}

#[tokio::test]
async fn json_mint_client_refuses_an_authorization_header_without_a_bearer_token() {
    let wire = Arc::new(CapturingWire::default());
    let client = sealing_client(wire.clone());
    let err = client
        .mint(&CallerHttpsAuth::new("Basic dXNlcjpwYXNz"))
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Mint(_)));
    assert!(wire.payloads().is_empty());
}

#[tokio::test]
async fn json_mint_client_seals_the_caller_token_to_the_callout_xkey() {
    let callout = harness_callout_xkey();
    let wire = Arc::new(CapturingWire::default());
    let client = AuthCalloutJsonMintClient::new(wire.clone(), "subject").sealing_to(callout.public_key());
    client
        .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
        .await
        .unwrap();

    let payloads = wire.payloads();
    assert_eq!(payloads.len(), 1);
    assert!(!String::from_utf8_lossy(&payloads[0]).contains("fixture-token"));
    let envelope: BridgeMintRequest = serde_json::from_slice(&payloads[0]).unwrap();
    let opened = envelope.sealed_credential.unwrap().open(&callout).unwrap();
    assert!(matches!(opened, BridgeCredential::BearerToken(token) if token.as_str() == "fixture-token"));
}
