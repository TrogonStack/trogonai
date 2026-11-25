use super::*;

#[tokio::test]
async fn reported_identity_must_equal_the_identity_in_the_authenticated_certificate() {
    let ca = TestCa::new();
    let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![identity.clone()]).await;
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let before = store.source_revision();
    fixture.replace(vec![identity.with_reported_id(PEER_ID)]);
    until(|| store.source_revision() > before).await;
    assert!(
        store.material().is_none(),
        "conflicting mandatory identity metadata must withdraw authority"
    );
    assert!(!lease.is_current());
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
