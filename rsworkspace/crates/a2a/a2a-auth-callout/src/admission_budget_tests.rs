use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::StreamExt as _;
use nkeys::{KeyPair, XKey};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::ReadEnv;

use crate::admission_keys::{AccountSigningKey, CalloutXkey};
use crate::constants::{ADMISSION_TEST_WAIT, AUTH_CALLOUT_SUBJECT, DEFAULT_BRIDGE_MINT_SUBJECT};
use crate::credentials::oidc::BearerToken;
use crate::dispatcher::AuthDispatcher;
use crate::error::{AuthCalloutError, CredentialError};
use crate::wire::ServerAuthRequestClaims;
use crate::wire::test_encode::FixtureServer;
use crate::{
    AdmissionBudget, AdmissionLimit, AdmissionLimitError, AuthCalloutWireCodec, BridgeCredential, BridgeMintReply,
    BridgeMintRequest, BridgeMintResponder, BridgeMintSubscriber, MintedUserJwt, SealedBridgeCredential, Subscriber,
};

struct FixtureEnv {
    readiness: String,
}

impl ReadEnv for FixtureEnv {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        match key {
            "AUTH_CALLOUT_READY_FILE" => Ok(self.readiness.clone()),
            _ => Err(std::env::VarError::NotPresent),
        }
    }
}

struct BlockingDispatcher {
    entered: mpsc::Sender<()>,
    release: Arc<Semaphore>,
    calls: Arc<AtomicUsize>,
    jwt: MintedUserJwt,
}

#[async_trait::async_trait]
impl AuthDispatcher for BlockingDispatcher {
    async fn dispatch(&self, _: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered
            .send(())
            .await
            .expect("the test observes each dispatch entry");
        let release = self
            .release
            .acquire()
            .await
            .map_err(|_| CredentialError::VerifierUnavailable { scheme: "blocked-test" })?;
        release.forget();
        Ok(self.jwt.clone())
    }
}

struct Fixture {
    _server: CoreTestServer,
    client: async_nats::Client,
    signer: FixtureServer,
    user: KeyPair,
    callout: KeyPair,
    xkey: CalloutXkey,
    entered: mpsc::Receiver<()>,
    release: Arc<Semaphore>,
    calls: Arc<AtomicUsize>,
    direct: JoinHandle<Result<(), AuthCalloutError>>,
    bridge: JoinHandle<()>,
    _readiness: tempfile::TempDir,
}

enum FixtureBudgets {
    DefaultConstructors,
    Shared(AdmissionBudget),
}

impl Fixture {
    async fn start() -> Self {
        Self::configured(FixtureBudgets::DefaultConstructors).await
    }

    async fn with_budget(budget: AdmissionBudget) -> Self {
        Self::configured(FixtureBudgets::Shared(budget)).await
    }

    async fn configured(budgets: FixtureBudgets) -> Self {
        let server = CoreTestServer::start().await;
        let client = async_nats::connect(server.address()).await.unwrap();
        let signer = FixtureServer::generate();
        let user = KeyPair::new_user();
        let callout = KeyPair::new_account();
        let xkey = CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap();
        let jwt = MintedUserJwt::new(
            nats_jwt_rs::user::User::new_claims("budget-test".into(), user.public_key())
                .encode(&callout)
                .unwrap(),
        )
        .unwrap();
        let (entered_tx, entered) = mpsc::channel(64);
        let release = Arc::new(Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = Arc::new(BlockingDispatcher {
            entered: entered_tx,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
            jwt,
        });
        let readiness = tempfile::tempdir().unwrap();
        let ready = readiness.path().join("ready");
        let env = FixtureEnv {
            readiness: ready.to_string_lossy().into_owned(),
        };
        let bridge = BridgeMintSubscriber::subscribe(
            client.clone(),
            BridgeMintResponder::new(Arc::clone(&dispatcher), Some(xkey.clone())),
            &env,
        )
        .await
        .unwrap();
        let bridge = match &budgets {
            FixtureBudgets::DefaultConstructors => bridge,
            FixtureBudgets::Shared(budget) => bridge.with_admission_budget(budget.clone()),
        };
        let bridge = tokio::spawn(bridge.serve());
        let wire = AuthCalloutWireCodec::new(
            signer.auth_account().clone(),
            AccountSigningKey::from_seed(&callout.seed().unwrap()).unwrap(),
            None,
        );
        let direct = Subscriber::new(client.clone(), dispatcher, wire);
        let direct = match budgets {
            FixtureBudgets::DefaultConstructors => direct,
            FixtureBudgets::Shared(budget) => direct.with_admission_budget(budget),
        };
        let direct = tokio::spawn(async move { direct.run(&env).await });
        tokio::time::timeout(ADMISSION_TEST_WAIT, async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both subscriptions are installed before readiness");
        client.flush().await.unwrap();
        Self {
            _server: server,
            client,
            signer,
            user,
            callout,
            xkey,
            entered,
            release,
            calls,
            direct,
            bridge,
            _readiness: readiness,
        }
    }

    fn direct_payload(&self) -> Vec<u8> {
        self.signer.sign(&self.user, |_| {}).into_bytes()
    }

    fn bridge_payload(&self) -> Vec<u8> {
        let credential = BridgeCredential::BearerToken(BearerToken::new("budget.fixture.token"));
        let request = BridgeMintRequest {
            account: Some(KeyPair::new_account().public_key()),
            sealed_credential: Some(SealedBridgeCredential::seal(&credential, &self.xkey.public_key()).unwrap()),
            ..BridgeMintRequest::default()
        };
        serde_json::to_vec(&request).unwrap()
    }

    async fn publish(&self, subject: &str, payload: Vec<u8>) -> async_nats::Subscriber {
        let inbox = self.client.new_inbox();
        let replies = self.client.subscribe(inbox.clone()).await.unwrap();
        self.client.flush().await.unwrap();
        self.client
            .publish_with_reply(subject.to_owned(), inbox, payload.into())
            .await
            .unwrap();
        replies
    }

    async fn entered(&mut self) {
        tokio::time::timeout(ADMISSION_TEST_WAIT, self.entered.recv())
            .await
            .unwrap()
            .unwrap();
    }

    async fn direct_overload(&mut self) {
        let client = self.client.clone();
        let request = client.request(AUTH_CALLOUT_SUBJECT, self.direct_payload().into());
        let response = tokio::time::timeout(ADMISSION_TEST_WAIT, async {
            tokio::select! {
                response = request => response.unwrap(),
                _ = self.entered.recv() => panic!("overflow reached the blocked dispatcher instead of being refused"),
            }
        })
        .await
        .expect("overload must be answered without waiting for blocked work");
        let claims = self.direct_claims(&response);
        assert_eq!(claims["nats"]["error"], "service_unavailable");
        assert!(claims["nats"]["jwt"].as_str().is_none_or(str::is_empty));
    }

    fn direct_claims(&self, response: &async_nats::Message) -> serde_json::Value {
        let jwt = std::str::from_utf8(&response.payload).unwrap();
        let (signed, signature) = jwt.rsplit_once('.').unwrap();
        self.callout
            .verify(signed.as_bytes(), &URL_SAFE_NO_PAD.decode(signature).unwrap())
            .unwrap();
        let (_, claims) = signed.split_once('.').unwrap();
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap()
    }

    async fn bridge_overload(&mut self) {
        let client = self.client.clone();
        let request = client.request(DEFAULT_BRIDGE_MINT_SUBJECT, self.bridge_payload().into());
        let response = tokio::time::timeout(ADMISSION_TEST_WAIT, async {
            tokio::select! {
                response = request => response.unwrap(),
                _ = self.entered.recv() => panic!("bridge overflow reached the blocked dispatcher instead of being refused"),
            }
        }).await.expect("bridge overload must not wait for blocked work");
        let reply: BridgeMintReply = serde_json::from_slice(&response.payload).unwrap();
        let BridgeMintReply::Refused(refusal) = reply else {
            panic!("overload must never return a broker JWT");
        };
        assert_eq!(refusal.refusal, "service_unavailable");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.release.close();
        self.direct.abort();
        self.bridge.abort();
    }
}

#[tokio::test]
async fn public_subscriber_defaults_refuse_work_beyond_the_finite_dispatch_budget() {
    let mut fixture = Fixture::start().await;
    let mut replies = Vec::new();
    for _ in 0..16 {
        replies.push(fixture.publish(AUTH_CALLOUT_SUBJECT, fixture.direct_payload()).await);
        fixture.entered().await;
    }
    fixture.direct_overload().await;
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 16);
    fixture.release.add_permits(16);
    for mut replies in replies {
        let response = tokio::time::timeout(ADMISSION_TEST_WAIT, replies.next())
            .await
            .unwrap()
            .unwrap();
        assert!(
            fixture.direct_claims(&response)["nats"]["jwt"]
                .as_str()
                .is_some_and(|jwt| !jwt.is_empty())
        );
    }
    let mut replies = Vec::new();
    for _ in 0..16 {
        replies.push(
            fixture
                .publish(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.bridge_payload())
                .await,
        );
        fixture.entered().await;
    }
    fixture.bridge_overload().await;
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 32);
    fixture.release.add_permits(16);
    for mut replies in replies {
        let response = tokio::time::timeout(ADMISSION_TEST_WAIT, replies.next())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            serde_json::from_slice::<BridgeMintReply>(&response.payload).unwrap(),
            BridgeMintReply::Minted(_)
        ));
    }
}

#[tokio::test]
async fn direct_and_bridge_subscribers_share_a_configured_budget_and_recover_after_release() {
    let budget = AdmissionBudget::new(AdmissionLimit::new(2).unwrap());
    let mut fixture = Fixture::with_budget(budget.clone()).await;
    let mut direct = fixture.publish(AUTH_CALLOUT_SUBJECT, fixture.direct_payload()).await;
    fixture.entered().await;
    let mut bridge = fixture
        .publish(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.bridge_payload())
        .await;
    fixture.entered().await;
    fixture.direct_overload().await;
    fixture.bridge_overload().await;
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
    fixture.release.add_permits(2);
    let response = tokio::time::timeout(ADMISSION_TEST_WAIT, direct.next())
        .await
        .unwrap()
        .unwrap();
    assert!(
        fixture.direct_claims(&response)["nats"]["jwt"]
            .as_str()
            .is_some_and(|jwt| !jwt.is_empty())
    );
    let response = tokio::time::timeout(ADMISSION_TEST_WAIT, bridge.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<BridgeMintReply>(&response.payload).unwrap(),
        BridgeMintReply::Minted(_)
    ));

    tokio::time::timeout(ADMISSION_TEST_WAIT, async {
        loop {
            let first = budget.try_acquire();
            let second = budget.try_acquire();
            if first.is_some() && second.is_some() {
                break;
            }
            drop((first, second));
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("completed publications release their admission permits");

    let mut next = fixture
        .publish(DEFAULT_BRIDGE_MINT_SUBJECT, fixture.bridge_payload())
        .await;
    fixture.entered().await;
    fixture.release.add_permits(1);
    let response = tokio::time::timeout(ADMISSION_TEST_WAIT, next.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<BridgeMintReply>(&response.payload).unwrap(),
        BridgeMintReply::Minted(_)
    ));
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn configured_budgets_reject_invalid_limits_and_release_held_capacity_with_the_guard() {
    assert_eq!(AdmissionLimit::new(0), Err(AdmissionLimitError::Zero));
    assert_eq!(AdmissionLimit::new(usize::MAX), Err(AdmissionLimitError::TooLarge));
    let budget = AdmissionBudget::new(AdmissionLimit::new(1).unwrap());
    let shared = budget.clone();
    let permit = budget.try_acquire().expect("one configured admission can enter");
    assert!(shared.try_acquire().is_none());
    drop(permit);
    assert!(shared.try_acquire().is_some());
}
