use super::*;

fn identity_with_octets(octets: usize) -> String {
    let prefix = "spiffe://example.org/";
    format!("{prefix}{}", "a".repeat(octets - prefix.len()))
}

#[test]
fn registration_identity_obeys_the_2048_octet_boundary() {
    assert!(RegistrationBinding::new(&identity_with_octets(2048), "tenant", "project", WorkloadRole::Supervisor, None).is_ok());
    assert_eq!(RegistrationBinding::new(&identity_with_octets(2049), "tenant", "project", WorkloadRole::Supervisor, None), Err(RegistrationError::Binding));
}

#[tokio::test]
async fn oversized_unselected_identity_withdraws_the_complete_snapshot() {
    let ca = TestCa::new();
    let local_identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![local_identity.clone(), ca.issue(&identity_with_octets(2048), Duration::from_secs(600))]).await;
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit.clone());
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let before = store.source_revision();
    fixture.replace(vec![local_identity, ca.issue(&identity_with_octets(2049), Duration::from_secs(600))]);
    until(|| store.source_revision() > before).await;
    assert!(store.material().is_none(), "an oversized unselected SVID invalidates the complete response");
    assert!(!lease.is_current());
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
