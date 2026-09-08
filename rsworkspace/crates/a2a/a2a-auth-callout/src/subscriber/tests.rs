use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nkeys::KeyPair;
use tokio::task::JoinHandle;
use trogon_nats::test_support::CoreTestServer;

use super::*;
use crate::error::CredentialError;
use crate::jwt::MintedUserJwt;
use crate::wire::test_encode::signed_auth_request;
use crate::wire::{NkeyPublic, NkeySeed, ServerAuthRequestClaims};

const WAIT: Duration = Duration::from_secs(5);

struct SubscriberEnv {
    readiness: PathBuf,
}

impl ReadEnv for SubscriberEnv {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        match key {
            "AUTH_CALLOUT_READY_FILE" => Ok(self.readiness.to_str().unwrap().to_owned()),
            "AUTH_CALLOUT_QUEUE_GROUP" => Ok("subscriber-contract".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        }
    }
}

enum DispatchOutcome {
    Grant,
    Refuse,
}

struct RecordingDispatcher {
    outcome: DispatchOutcome,
    jwt: MintedUserJwt,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl AuthDispatcher for RecordingDispatcher {
    async fn dispatch(&self, _request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.outcome {
            DispatchOutcome::Grant => Ok(self.jwt.clone()),
            DispatchOutcome::Refuse => {
                Err(CredentialError::InvalidCredentials("verifier diagnostic detail".into()).into())
            }
        }
    }
}

struct SubscriberFixture {
    _server: CoreTestServer,
    client: async_nats::Client,
    signer: KeyPair,
    callout: KeyPair,
    user: KeyPair,
    minted: MintedUserJwt,
    calls: Arc<AtomicUsize>,
    task: JoinHandle<Result<(), AuthCalloutError>>,
    readiness: tempfile::TempDir,
}

impl SubscriberFixture {
    async fn new(outcome: DispatchOutcome) -> Self {
        let server = CoreTestServer::start().await;
        let client = async_nats::connect(server.address()).await.unwrap();
        let signer = KeyPair::new_server();
        let callout = KeyPair::new_account();
        let user = KeyPair::new_user();
        let minted = MintedUserJwt::new(
            nats_jwt_rs::user::User::new_claims("subscriber-test".into(), user.public_key())
                .encode(&callout)
                .unwrap(),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let readiness = tempfile::tempdir().unwrap();
        let task = start_subscriber(
            &client,
            &signer,
            &callout,
            RecordingDispatcher {
                outcome,
                jwt: minted.clone(),
                calls: calls.clone(),
            },
            &readiness.path().join("primary/ready"),
        )
        .await;
        Self {
            _server: server,
            client,
            signer,
            callout,
            user,
            minted,
            calls,
            task,
            readiness,
        }
    }

    fn request_token(&self) -> String {
        signed_auth_request(&self.signer, &self.user, |_| {})
    }

    async fn request(&self, token: String) -> async_nats::Message {
        tokio::time::timeout(WAIT, self.client.request(AUTH_CALLOUT_SUBJECT, token.into()))
            .await
            .unwrap()
            .unwrap()
    }

    async fn stop(self) {
        self.client.drain().await.unwrap();
        tokio::time::timeout(WAIT, self.task).await.unwrap().unwrap().unwrap();
    }
}

async fn start_subscriber(
    client: &async_nats::Client,
    signer: &KeyPair,
    callout: &KeyPair,
    dispatcher: RecordingDispatcher,
    readiness: &Path,
) -> JoinHandle<Result<(), AuthCalloutError>> {
    let wire = AuthCalloutWireCodec::new(
        NkeyPublic::parse(signer.public_key()).unwrap(),
        NkeySeed::parse(callout.seed().unwrap()).unwrap(),
        None,
        None,
    )
    .unwrap();
    let subscriber = Subscriber::new(client.clone(), dispatcher, wire);
    let env = SubscriberEnv {
        readiness: readiness.to_owned(),
    };
    let task = tokio::spawn(async move { subscriber.run(&env).await });
    tokio::time::timeout(WAIT, async {
        loop {
            if std::fs::read(readiness).is_ok_and(|bytes| bytes == b"ready\n") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("subscriber writes readiness after installing its subscription");
    client.flush().await.unwrap();
    task
}

fn verified_response(message: &async_nats::Message, callout: &KeyPair) -> serde_json::Value {
    let jwt = std::str::from_utf8(&message.payload).unwrap();
    let (signed, encoded_signature) = jwt.rsplit_once('.').unwrap();
    let signature = URL_SAFE_NO_PAD.decode(encoded_signature).unwrap();
    callout.verify(signed.as_bytes(), &signature).unwrap();
    let (_, encoded_payload) = signed.split_once('.').unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded_payload).unwrap()).unwrap()
}

#[tokio::test]
async fn subscriber_signs_grants_and_discards_requests_without_a_reply() {
    let fixture = SubscriberFixture::new(DispatchOutcome::Grant).await;
    fixture
        .client
        .publish(AUTH_CALLOUT_SUBJECT, fixture.request_token().into())
        .await
        .unwrap();
    fixture.client.flush().await.unwrap();
    let reply = fixture.request(fixture.request_token()).await;
    let claims = verified_response(&reply, &fixture.callout);
    assert_eq!(claims["sub"], fixture.user.public_key());
    assert_eq!(claims["aud"], fixture.signer.public_key());
    assert_eq!(claims["iss"], fixture.callout.public_key());
    assert_eq!(claims["nats"]["jwt"], fixture.minted.as_str());
    assert!(claims["nats"]["error"].as_str().unwrap_or_default().is_empty());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    fixture.stop().await;
}

#[tokio::test]
async fn subscriber_denial_exposes_only_the_opaque_category() {
    let fixture = SubscriberFixture::new(DispatchOutcome::Refuse).await;
    let reply = fixture.request(fixture.request_token()).await;
    let claims = verified_response(&reply, &fixture.callout);
    assert_eq!(claims["nats"]["error"], "invalid_credentials");
    assert!(claims["nats"]["jwt"].as_str().unwrap_or_default().is_empty());
    assert_eq!(claims["sub"], fixture.user.public_key());
    assert_eq!(claims["aud"], fixture.signer.public_key());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    fixture.stop().await;
}

#[tokio::test]
async fn subscriber_rejects_malformed_and_untrusted_requests_without_dispatching() {
    let fixture = SubscriberFixture::new(DispatchOutcome::Grant).await;
    let untrusted = signed_auth_request(&KeyPair::new_server(), &fixture.user, |_| {});
    for payload in ["malformed-request".to_owned(), untrusted] {
        assert!(fixture.request(payload).await.payload.is_empty());
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    fixture.stop().await;
}

#[tokio::test]
async fn subscriber_returns_empty_fallback_when_response_encryption_is_unavailable() {
    let fixture = SubscriberFixture::new(DispatchOutcome::Grant).await;
    let request = signed_auth_request(&fixture.signer, &fixture.user, |claims| {
        claims.nats.server.xkey = Some(nkeys::XKey::new().public_key());
    });
    assert!(fixture.request(request).await.payload.is_empty());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    fixture.stop().await;
}

#[tokio::test]
async fn replicas_in_one_queue_dispatch_each_authorization_request_once() {
    let fixture = SubscriberFixture::new(DispatchOutcome::Grant).await;
    let replica = start_subscriber(
        &fixture.client,
        &fixture.signer,
        &fixture.callout,
        RecordingDispatcher {
            outcome: DispatchOutcome::Grant,
            jwt: fixture.minted.clone(),
            calls: fixture.calls.clone(),
        },
        &fixture.readiness.path().join("replica/ready"),
    )
    .await;
    let inbox = fixture.client.new_inbox();
    let mut replies = fixture.client.subscribe(inbox.clone()).await.unwrap();
    fixture.client.flush().await.unwrap();
    for _ in 0..16 {
        fixture
            .client
            .publish_with_reply(AUTH_CALLOUT_SUBJECT, inbox.clone(), fixture.request_token().into())
            .await
            .unwrap();
    }
    for _ in 0..16 {
        let reply = tokio::time::timeout(WAIT, replies.next()).await.unwrap().unwrap();
        assert_eq!(
            verified_response(&reply, &fixture.callout)["nats"]["jwt"],
            fixture.minted.as_str()
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), replies.next())
            .await
            .is_err()
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 16);
    fixture.stop().await;
    tokio::time::timeout(WAIT, replica).await.unwrap().unwrap().unwrap();
}
