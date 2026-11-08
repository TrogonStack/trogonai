use std::time::Duration;

use nkeys::{KeyPair, XKey};
use serde_json::json;
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::InMemoryEnv;

use super::*;
use crate::admission_keys::CalloutXkey;
use crate::bridge_mint::{BridgeCredential, BridgeMintReply, BridgeMintRequest, SealedBridgeCredential};
use crate::constants::{BRIDGE_MINT_SUBJECT_VAR, DEFAULT_BRIDGE_MINT_SUBJECT};
use crate::credentials::oidc::BearerToken;
use crate::jwt::MintedUserJwt;
use crate::wire::ServerAuthRequestClaims;

const WAIT: Duration = Duration::from_secs(5);

enum Outcome {
    Grant(MintedUserJwt),
    Refuse,
}

struct ScriptedDispatcher(Outcome);

#[async_trait::async_trait]
impl AuthDispatcher for ScriptedDispatcher {
    async fn dispatch(&self, _request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        match &self.0 {
            Outcome::Grant(jwt) => Ok(jwt.clone()),
            Outcome::Refuse => Err(crate::error::CredentialError::InvalidCredentials("detail".into()).into()),
        }
    }
}

fn jwt(signature: &str) -> MintedUserJwt {
    MintedUserJwt::new(format!("header.payload.{signature}")).unwrap()
}

struct Fixture {
    _server: CoreTestServer,
    client: async_nats::Client,
    callout: CalloutXkey,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(outcome: Outcome) -> Self {
        Self::on_subject(outcome, None).await
    }

    async fn on_subject(outcome: Outcome, subject: Option<&str>) -> Self {
        let server = CoreTestServer::start().await;
        let client = async_nats::connect(server.address()).await.unwrap();
        let callout = CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap();
        let env = InMemoryEnv::new();
        if let Some(subject) = subject {
            env.set(BRIDGE_MINT_SUBJECT_VAR, subject);
        }
        let responder = BridgeMintResponder::new(ScriptedDispatcher(outcome), Some(callout.clone()));
        let subscriber = BridgeMintSubscriber::subscribe(client.clone(), responder, &env)
            .await
            .unwrap();
        assert_eq!(subscriber.subject(), subject.unwrap_or(DEFAULT_BRIDGE_MINT_SUBJECT));
        let task = tokio::spawn(subscriber.serve());
        client.flush().await.unwrap();
        Self {
            _server: server,
            client,
            callout,
            task,
        }
    }

    fn payload(&self) -> Vec<u8> {
        let credential = BridgeCredential::BearerToken(BearerToken::new("caller.bearer.token"));
        let request = BridgeMintRequest {
            account: Some(KeyPair::new_account().public_key()),
            sealed_credential: Some(SealedBridgeCredential::seal(&credential, &self.callout.public_key()).unwrap()),
            ..BridgeMintRequest::default()
        };
        serde_json::to_vec(&request).unwrap()
    }

    async fn request(&self, subject: &str, payload: Vec<u8>) -> async_nats::Message {
        tokio::time::timeout(WAIT, self.client.request(subject.to_owned(), payload.into()))
            .await
            .unwrap()
            .unwrap()
    }

    async fn stop(self) {
        self.client.drain().await.unwrap();
        tokio::time::timeout(WAIT, self.task).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn a_bridge_request_on_the_default_subject_is_answered_with_the_minted_jwt() {
    let fixture = Fixture::new(Outcome::Grant(jwt("minted"))).await;
    let reply = fixture.request(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.payload()).await;
    let BridgeMintReply::Minted(minted) = serde_json::from_slice(&reply.payload).unwrap() else {
        panic!("expected a minted JWT");
    };
    assert_eq!(minted.user_jwt, "header.payload.minted");
    fixture.stop().await;
}

#[tokio::test]
async fn a_refused_or_malformed_request_is_answered_with_a_refusal_and_never_a_jwt() {
    let fixture = Fixture::new(Outcome::Refuse).await;
    let refused = fixture.request(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.payload()).await;
    assert_eq!(refused.payload.as_ref(), br#"{"refusal":"invalid_credentials"}"#);
    let malformed = fixture.request(DEFAULT_BRIDGE_MINT_SUBJECT, b"not json".to_vec()).await;
    assert_eq!(malformed.payload.as_ref(), br#"{"refusal":"invalid_request"}"#);
    fixture.stop().await;
}

#[tokio::test]
async fn the_configured_subject_replaces_the_default() {
    let fixture = Fixture::on_subject(Outcome::Grant(jwt("minted")), Some("acme.bridge.mint")).await;
    let reply = fixture.request("acme.bridge.mint", fixture.payload()).await;
    assert!(serde_json::from_slice::<BridgeMintReply>(&reply.payload).is_ok());
    let unanswered = tokio::time::timeout(
        Duration::from_millis(300),
        fixture
            .client
            .request(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.payload().into()),
    )
    .await;
    assert!(matches!(unanswered, Ok(Err(_)) | Err(_)));
    fixture.stop().await;
}

#[tokio::test]
async fn a_request_without_a_reply_subject_is_dropped_and_the_next_one_is_answered() {
    let fixture = Fixture::new(Outcome::Grant(jwt("minted"))).await;
    fixture
        .client
        .publish(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.payload().into())
        .await
        .unwrap();
    let reply = fixture.request(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.payload()).await;
    assert!(serde_json::from_slice::<BridgeMintReply>(&reply.payload).is_ok());
    fixture.stop().await;
}

#[tokio::test]
async fn a_reply_the_server_cannot_carry_leaves_the_requester_unanswered() {
    let oversized = jwt(&"s".repeat(2 * 1024 * 1024));
    let fixture = Fixture::new(Outcome::Grant(oversized)).await;
    let unanswered = tokio::time::timeout(
        Duration::from_millis(500),
        fixture
            .client
            .request(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.payload().into()),
    )
    .await;
    assert!(unanswered.is_err());
    fixture.stop().await;
}

#[test]
fn a_reply_serializes_as_its_variant_alone() {
    let minted = BridgeMintReply::Minted(crate::bridge_mint::BridgeMintResponse {
        user_jwt: "a.b.c".to_owned(),
    });
    assert_eq!(serde_json::to_value(&minted).unwrap(), json!({"user_jwt": "a.b.c"}));
}
