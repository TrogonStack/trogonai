use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tokio::sync::Semaphore;

use crate::constants::TEST_TIMEOUT;
use crate::test_support::{TestCa, WorkloadApiFixture};
use crate::*;

const LOCAL_ID: &str = "spiffe://example.org/service/egress";
const PEER_ID: &str = "spiffe://example.org/runtime/workload-1";

#[derive(Default)]
struct Recorder(Mutex<Vec<ActivationFact>>);

#[async_trait]
impl ActivationAudit for Recorder {
    async fn record(&self, fact: &ActivationFact) -> Result<(), ActivationAuditError> {
        self.0.lock().unwrap().push(fact.clone());
        Ok(())
    }
}

fn local() -> RegistrationBinding { RegistrationBinding::new(LOCAL_ID, "tenant", "project", WorkloadRole::EgressProxy, None).unwrap() }
fn peer() -> RegistrationBinding { RegistrationBinding::new(PEER_ID, "tenant", "project", WorkloadRole::Agent, Some("agent-1")).unwrap() }
fn revision(value: u64) -> NonZeroU64 { NonZeroU64::new(value).unwrap() }
fn registration(ca: &TestCa) -> RegistrationSet {
    RegistrationSet::new(revision(1), revision(1), revision(1), SystemTime::now() + Duration::from_secs(300), vec![local(), peer()], vec![ca.root_sha256()]).unwrap()
}

async fn material(store: &IdentityStore) -> MaterialLease {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(material) = store.material() { return material; }
            tokio::task::yield_now().await;
        }
    }).await.expect("verified material must activate")
}

#[tokio::test]
async fn supported_sdk_reads_real_unix_workload_stream() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let client = spiffe::WorkloadApiClient::connect(fixture.endpoint().endpoint()).await.unwrap();
    let mut stream = client.stream_x509_contexts().await.unwrap();
    let context = tokio::time::timeout(TEST_TIMEOUT, stream.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(context.svids()[0].spiffe_id().to_string(), LOCAL_ID);
    assert_eq!(fixture.requests(), 1);
    fixture.permission_denied();
    assert!(matches!(tokio::time::timeout(TEST_TIMEOUT, stream.next()).await.unwrap().unwrap(), Err(spiffe::WorkloadApiError::PermissionDenied(_))));
    fixture.stop().await;
}

#[tokio::test]
async fn verified_material_activates_only_with_exact_registration_and_durable_record() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit.clone());
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    assert!(lease.is_current());
    assert_eq!(lease.application_for_peer(&spiffe::SpiffeId::new(PEER_ID).unwrap()).unwrap().as_str(), "agent-1");
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    store.withdraw_registration();
    assert!(!lease.is_current());
    assert!(store.material().is_none());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

struct HeldAudit {
    entered: AtomicUsize,
    permits: Semaphore,
}

#[async_trait]
impl ActivationAudit for HeldAudit {
    async fn record(&self, _: &ActivationFact) -> Result<(), ActivationAuditError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.permits.acquire().await.map_err(|_| ActivationAuditError::Unavailable)?.forget();
        Ok(())
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        while !condition() { tokio::task::yield_now().await; }
    }).await.expect("state transition");
}

#[tokio::test]
async fn permission_withdrawal_is_observed_while_activation_audit_is_pending() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let audit = Arc::new(HeldAudit { entered: AtomicUsize::new(0), permits: Semaphore::new(0) });
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit.clone());
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    until(|| audit.entered.load(Ordering::SeqCst) == 1).await;
    assert!(store.material().is_none());
    let source = store.source_revision();
    fixture.permission_denied();
    until(|| store.source_revision() > source).await;
    audit.permits.add_permits(1);
    assert!(store.material().is_none());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn selecting_exact_identity_does_not_follow_workload_api_default_order() {
    let ca = TestCa::new();
    let local_identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![ca.issue(PEER_ID, Duration::from_secs(600)), local_identity.clone()]).await;
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit.clone());
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    assert_eq!(audit.0.lock().unwrap()[0].binding(), &local());
    fixture.replace(vec![ca.issue(PEER_ID, Duration::from_secs(600))]);
    until(|| !lease.is_current()).await;
    assert!(store.material().is_none());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn invalid_key_unapproved_root_and_excessive_lifetime_never_activate() {
    let ca = TestCa::new();
    let wrong_ca = TestCa::new();
    let valid = ca.issue(LOCAL_ID, Duration::from_secs(600));
    for identity in [
        valid.clone().with_private_key(&ca.issue(LOCAL_ID, Duration::from_secs(600))),
        wrong_ca.issue(LOCAL_ID, Duration::from_secs(600)),
        ca.issue(LOCAL_ID, Duration::from_secs(7200)),
        valid.with_bundle(&[]),
    ] {
        let fixture = WorkloadApiFixture::start(vec![identity]).await;
        let audit = Arc::new(Recorder::default());
        let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit.clone());
        store.replace_registration(registration(&ca)).unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(driver.run(shutdown.clone()));
        until(|| store.source_revision() > 0).await;
        assert!(store.material().is_none());
        assert!(audit.0.lock().unwrap().is_empty());
        shutdown.cancel();
        task.await.unwrap();
        fixture.stop().await;
    }
}
