use super::*;

#[tokio::test]
async fn supplied_crls_never_activate_and_withdraw_cached_authority() {
    let ca = TestCa::new();
    let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![identity.clone()]).await;
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let before = store.source_revision();
    fixture.replace_with_crls(vec![identity.clone()], vec![vec![0x30, 0x00]]);
    until(|| store.source_revision() > before).await;
    assert!(!lease.is_current(), "supplied revocation material cannot be silently ignored");
    assert!(store.material().is_none());
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    until(|| fixture.requests() >= 2).await;
    store.replace_registration(RegistrationSet::new(
        revision(1), revision(2), revision(1), original.expires_at(),
        original.bindings().to_vec(), original.approved_roots().to_vec(),
    ).unwrap()).unwrap();
    let before = store.source_revision();
    until(|| store.source_revision() > before).await;
    assert!(store.material().is_none(), "a fresh registration cannot authorize unsupported CRLs");
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    fixture.replace(vec![identity]);
    let before = store.source_revision();
    until(|| store.source_revision() > before).await;
    store.replace_registration(RegistrationSet::new(
        revision(1), revision(3), revision(1), original.expires_at(),
        original.bindings().to_vec(), original.approved_roots().to_vec(),
    ).unwrap()).unwrap();
    assert_eq!(material(&store).await.fact().registration_revision(), revision(3));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
