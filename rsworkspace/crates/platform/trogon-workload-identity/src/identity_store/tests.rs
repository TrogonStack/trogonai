use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::constants::TEST_TIMEOUT;
use crate::test_support::{TestCa, WorkloadApiFixture};
use crate::tests::{LOCAL_ID, PEER_ID, Recorder, handshake, local, material, peer, registration, revision, until};
use crate::{ActivationAuditError, ActivationFact};

#[tokio::test]
async fn source_counter_exhaustion_withdraws_instead_of_reusing_an_activation_generation() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    store.inner.state.lock().unwrap().source_revision = u64::MAX;
    fixture.replace(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]);
    tokio::time::timeout(TEST_TIMEOUT, lease.changed()).await.unwrap();
    assert!(store.material().is_none());
    let next = RegistrationSet::new(
        revision(1),
        revision(2),
        revision(1),
        original.expires_at(),
        original.bindings().to_vec(),
        original.approved_roots().to_vec(),
    )
    .unwrap();
    assert_eq!(store.replace_registration(next), Err(RegistrationError::Unavailable));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn withdrawal_and_observation_counter_exhaustion_both_seal_the_store() {
    let ca = TestCa::new();
    for source in [true, false] {
        let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
        let (store, driver) = IdentityStore::new(
            IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
            Arc::new(Recorder::default()),
        );
        let original = registration(&ca);
        store.replace_registration(original.clone()).unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(driver.run(shutdown.clone()));
        let lease = material(&store).await;
        if source {
            store.inner.state.lock().unwrap().source_revision = u64::MAX;
            fixture.permission_denied();
            tokio::time::timeout(TEST_TIMEOUT, lease.changed()).await.unwrap();
        } else {
            store.inner.state.lock().unwrap().epoch = u64::MAX;
            store.withdraw_registration();
        }
        assert!(!lease.is_current());
        assert!(store.prepare().is_none());
        assert_eq!(
            store.replace_registration(original),
            Err(RegistrationError::Unavailable)
        );
        shutdown.cancel();
        task.await.unwrap();
        fixture.stop().await;
    }
}

struct RevokingAudit {
    inner: Mutex<Option<Weak<Inner>>>,
    calls: AtomicUsize,
}

#[async_trait]
impl ActivationAudit for RevokingAudit {
    async fn record(&self, _: &ActivationFact) -> Result<(), ActivationAuditError> {
        if let Some(inner) = self.inner.lock().unwrap().as_ref().and_then(Weak::upgrade) {
            IdentityStore { inner }.withdraw_registration();
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn revocation_inside_the_durable_ack_callback_wins_the_final_atomic_activation_check() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let audit = Arc::new(RevokingAudit {
        inner: Mutex::new(None),
        calls: AtomicUsize::new(0),
    });
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
    *audit.inner.lock().unwrap() = Some(Arc::downgrade(&store.inner));
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    until(|| audit.calls.load(Ordering::SeqCst) == 1).await;
    assert!(store.material().is_none());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn poisoned_host_state_cannot_retain_or_reactivate_a_key() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let poison = store.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = poison.inner.state.lock().unwrap();
            panic!("simulate an interrupted host mutation");
        })
        .join()
        .is_err()
    );
    assert!(!lease.is_current());
    assert!(store.material().is_none());
    assert_eq!(
        store.replace_registration(original),
        Err(RegistrationError::Unavailable)
    );
    tokio::time::timeout(TEST_TIMEOUT, lease.changed()).await.unwrap();
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[test]
fn poisoned_host_state_refuses_late_source_and_durable_activation_callbacks() {
    let ca = TestCa::new();
    let candidate = || {
        let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
        let wire = crate::workload_response_wire::WorkloadResponseWire {
            svids: vec![crate::workload_identity_wire::WorkloadIdentityWire {
                spiffe_id: LOCAL_ID.into(),
                x509_svid: identity.certificates()[0].to_vec(),
                x509_svid_key: identity.private_key().secret_der().to_vec(),
                bundle: ca.root().to_vec(),
                hint: String::new(),
            }],
            crls: Vec::new(),
            federated_bundles: Default::default(),
        };
        Candidate::parse(
            crate::workload_context::WorkloadContext::from_wire(wire).unwrap(),
            local().spiffe_id(),
        )
        .unwrap()
    };
    let (store, _driver) = IdentityStore::new(
        IdentityConfig::new(
            crate::UnixEndpoint::new("unix:///nonexistent-workload.sock").unwrap(),
            local(),
            revision(1),
        ),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    store.observe(candidate());
    let pending = store.prepare().expect("fully validated pending audit activation");
    let late_source = candidate();
    let poison = store.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = poison.inner.state.lock().unwrap();
            panic!("simulate host interruption after validation");
        })
        .join()
        .is_err()
    );
    store.activate(pending);
    store.observe(late_source);
    assert!(store.material().is_none());
    assert!(store.prepare().is_none());
    assert_eq!(store.source_revision(), u64::MAX);
}

#[tokio::test]
async fn monotonic_expiry_retires_active_material_and_notifies_without_source_withdrawal() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![
        ca.issue(LOCAL_ID, Duration::from_secs(600)),
        ca.issue(PEER_ID, Duration::from_secs(600)),
    ])
    .await;
    let audit = Arc::new(Recorder::default());
    let (server, server_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
    let (client, client_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), peer(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let original = registration(&ca);
    let short = RegistrationSet::new(
        revision(1),
        revision(1),
        revision(1),
        SystemTime::now() + Duration::from_secs(30),
        original.bindings().to_vec(),
        original.approved_roots().to_vec(),
    )
    .unwrap();
    server.replace_registration(short.clone()).unwrap();
    client.replace_registration(original).unwrap();
    let shutdown = CancellationToken::new();
    let server_task = tokio::spawn(server_driver.run(shutdown.clone()));
    let client_task = tokio::spawn(client_driver.run(shutdown.clone()));
    let server_lease = material(&server).await;
    let client_lease = material(&client).await;
    let cached_server = server_lease.server_config();
    let cached_client = client_lease.client_config(local().spiffe_id()).unwrap();
    assert!(handshake(cached_server.clone(), cached_client.clone()).is_ok());
    let mut events = server.inner.events.subscribe();
    let before = *events.borrow_and_update();
    let source = server.source_revision();
    server.expire();
    assert!(!events.has_changed().unwrap());

    tokio::time::pause();
    tokio::time::advance(server_lease.deadline().saturating_duration_since(Instant::now()) + Duration::from_nanos(1))
        .await;
    server.expire();
    assert!(events.has_changed().unwrap());
    events.changed().await.unwrap();
    assert_eq!(*events.borrow_and_update(), before + 1);
    assert_eq!(server.source_revision(), source);
    assert!(short.expires_at() > SystemTime::now());
    assert_eq!(audit.count(), 1);
    assert!(server.material().is_none());
    assert!(!server_lease.is_current());
    assert!(client_lease.is_current());
    assert!(handshake(cached_server, cached_client).is_err());
    server.expire();
    assert!(
        !events.has_changed().unwrap(),
        "retired material cannot emit another cleanup event"
    );

    tokio::time::resume();
    shutdown.cancel();
    server_task.await.unwrap();
    client_task.await.unwrap();
    fixture.stop().await;
}
