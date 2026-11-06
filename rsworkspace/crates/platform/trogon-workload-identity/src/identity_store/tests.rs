use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::constants::TEST_TIMEOUT;
use crate::test_support::{TestCa, WorkloadApiFixture};
use crate::tests::{LOCAL_ID, Recorder, local, material, registration, revision, until};
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
