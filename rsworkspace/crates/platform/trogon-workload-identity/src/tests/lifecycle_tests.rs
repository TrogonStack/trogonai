use super::*;

#[tokio::test]
async fn reconnect_cannot_reactivate_a_withdrawn_snapshot_without_new_governed_authority() {
    let ca = TestCa::new();
    let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![identity.clone()]).await;
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), Arc::new(Recorder::default()));
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let source = store.source_revision();
    fixture.permission_denied();
    until(|| !lease.is_current() && store.source_revision() > source).await;
    let withdrawn = store.source_revision();
    fixture.replace(vec![identity]);
    until(|| store.source_revision() > withdrawn && fixture.requests() >= 2).await;
    assert!(store.material().is_none());
    store.replace_registration(RegistrationSet::new(revision(1), revision(2), revision(1), original.expires_at(), original.bindings().to_vec(), original.approved_roots().to_vec()).unwrap()).unwrap();
    let fresh = material(&store).await;
    assert!(fresh.fact().source_revision() > lease.fact().source_revision());
    assert_eq!(fresh.fact().registration_revision(), revision(2));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn root_removal_withdraws_old_material_while_replacement_audit_waits() {
    let ca = TestCa::new();
    let next_ca = TestCa::new();
    let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![identity]).await;
    let audit = Arc::new(HeldAudit { entered: AtomicUsize::new(0), permits: Semaphore::new(1) });
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit.clone());
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let next = RegistrationSet::new(revision(1), revision(2), revision(2), original.expires_at(), original.bindings().to_vec(), vec![next_ca.root_sha256()]).unwrap();
    store.replace_registration(next).unwrap();
    fixture.replace(vec![next_ca.issue(LOCAL_ID, Duration::from_secs(600))]);
    until(|| audit.entered.load(Ordering::SeqCst) == 2).await;
    assert!(!lease.is_current());
    tokio::time::timeout(TEST_TIMEOUT, lease.changed()).await.unwrap();
    assert!(store.material().is_none());
    audit.permits.add_permits(1);
    let fresh = material(&store).await;
    assert_eq!(fresh.fact().trust_revision(), revision(2));
    assert_ne!(fresh.fact().bundle_sha256(), lease.fact().bundle_sha256());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn registration_equivocation_and_wrong_scope_withdraw_before_a_newer_valid_projection() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), Arc::new(Recorder::default()));
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    store.replace_registration(original.clone()).unwrap();
    assert_eq!(material(&store).await.deadline(), lease.deadline());
    let equivocation = RegistrationSet::new(revision(1), revision(1), revision(1), original.expires_at() + Duration::from_secs(1), original.bindings().to_vec(), original.approved_roots().to_vec()).unwrap();
    assert_eq!(store.replace_registration(equivocation), Err(RegistrationError::Revision));
    assert!(!lease.is_current());
    let wrong_scope = RegistrationSet::new(revision(1), revision(2), revision(1), original.expires_at(), vec![local(), RegistrationBinding::new(PEER_ID, "other", "project", WorkloadRole::Agent, Some("agent-1")).unwrap()], original.approved_roots().to_vec()).unwrap();
    assert_eq!(store.replace_registration(wrong_scope), Err(RegistrationError::Scope));
    let wrong_fence = RegistrationSet::new(revision(2), revision(2), revision(1), original.expires_at(), original.bindings().to_vec(), original.approved_roots().to_vec()).unwrap();
    assert_eq!(store.replace_registration(wrong_fence), Err(RegistrationError::Scope));
    let next = RegistrationSet::new(revision(1), revision(2), revision(1), original.expires_at(), original.bindings().to_vec(), original.approved_roots().to_vec()).unwrap();
    store.replace_registration(next).unwrap();
    assert!(material(&store).await.is_current());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn an_expired_or_overlong_registration_cannot_preserve_existing_material() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), Arc::new(Recorder::default()));
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    for expiry in [SystemTime::UNIX_EPOCH, SystemTime::now() + Duration::from_secs(331)] {
        let invalid = RegistrationSet::new(revision(1), revision(2), revision(1), expiry, original.bindings().to_vec(), original.approved_roots().to_vec()).unwrap();
        assert_eq!(store.replace_registration(invalid), Err(RegistrationError::Freshness));
        assert!(store.material().is_none());
        assert!(!lease.is_current());
    }
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
