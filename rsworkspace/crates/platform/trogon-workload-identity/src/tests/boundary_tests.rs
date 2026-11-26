use std::collections::BTreeMap;

use super::*;

#[test]
fn trust_domain_identifiers_cannot_be_registered_as_workloads() {
    assert_eq!(
        RegistrationBinding::new(
            "spiffe://example.org",
            "tenant",
            "project",
            WorkloadRole::Supervisor,
            None
        ),
        Err(RegistrationError::Binding)
    );
}

#[tokio::test]
async fn an_unavailable_workload_socket_cannot_activate_governed_authority() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = UnixEndpoint::new(&format!("unix://{}", directory.path().join("missing.sock").display())).unwrap();
    let ca = TestCa::new();
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(IdentityConfig::new(endpoint, local(), revision(1)), audit.clone());
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    until(|| store.source_revision() > 0).await;
    assert!(store.material().is_none());
    assert!(audit.0.lock().unwrap().is_empty());
    shutdown.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn excessive_combined_trust_domains_withdraw_the_complete_workload_snapshot() {
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
    fixture.replace_with_federated_bundles(
        vec![identity],
        (0..crate::constants::MAX_ROOTS)
            .map(|index| (format!("spiffe://foreign-{index}.example"), ca.root().to_vec()))
            .collect::<BTreeMap<_, _>>(),
    );
    until(|| !lease.is_current()).await;
    assert!(store.material().is_none());
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn missing_reported_identity_withdraws_previous_material_before_audit() {
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
    fixture.replace(vec![identity.with_reported_id("")]);
    until(|| !lease.is_current()).await;
    assert!(store.material().is_none());
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn an_empty_initial_snapshot_cannot_reach_the_activation_audit() {
    let fixture = WorkloadApiFixture::start(Vec::new()).await;
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
    store.replace_registration(registration(&TestCa::new())).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    until(|| store.source_revision() > 0 && fixture.requests() > 0).await;
    assert!(store.material().is_none());
    assert!(audit.0.lock().unwrap().is_empty());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn a_registered_peer_name_cannot_substitute_an_untrusted_signing_chain() {
    let ca = TestCa::new();
    let foreign_ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let impostor = foreign_ca.issue(PEER_ID, Duration::from_secs(600));
    assert!(matches!(
        lease.peer_evidence_from_certificates(&impostor.certificates()),
        Err(MaterialError::Certificate)
    ));
    assert!(lease.is_current());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
