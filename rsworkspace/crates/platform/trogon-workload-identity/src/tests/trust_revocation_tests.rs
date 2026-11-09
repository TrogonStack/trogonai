use super::*;

#[tokio::test]
async fn complete_trust_withdrawal_retains_its_revision_floor() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let original = registration(&ca);
    let snapshot = |value, trust, roots| {
        RegistrationSet::new(
            revision(1),
            revision(value),
            revision(trust),
            original.expires_at(),
            original.bindings().to_vec(),
            roots,
        )
    };
    store
        .replace_registration(snapshot(2, 2, original.approved_roots().to_vec()).unwrap())
        .unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let revoked = snapshot(100, 100, Vec::new());
    assert!(
        revoked.is_ok(),
        "an empty approved root set is a complete trust withdrawal"
    );
    store.replace_registration(revoked.unwrap()).unwrap();
    assert!(!lease.is_current());
    assert!(store.material().is_none());
    assert_eq!(
        store.replace_registration(snapshot(50, 50, original.approved_roots().to_vec()).unwrap()),
        Err(RegistrationError::Revision)
    );
    assert_eq!(
        store.replace_registration(snapshot(101, 50, original.approved_roots().to_vec()).unwrap()),
        Err(RegistrationError::Revision)
    );
    assert_eq!(
        store.replace_registration(snapshot(101, 100, original.approved_roots().to_vec()).unwrap()),
        Err(RegistrationError::Revision)
    );
    store
        .replace_registration(snapshot(101, 101, original.approved_roots().to_vec()).unwrap())
        .unwrap();
    assert_eq!(material(&store).await.fact().trust_revision(), revision(101));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
