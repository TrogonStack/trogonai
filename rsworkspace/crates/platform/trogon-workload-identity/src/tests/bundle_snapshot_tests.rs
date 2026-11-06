use std::collections::BTreeMap;

use super::*;

enum Ambiguity { PerSvid, FederatedLocal }

#[tokio::test]
async fn conflicting_same_domain_bundles_withdraw_authority() {
    assert_ambiguous_snapshot_withdraws(Ambiguity::PerSvid).await;
}

#[tokio::test]
async fn federated_local_override_withdraws_authority() {
    assert_ambiguous_snapshot_withdraws(Ambiguity::FederatedLocal).await;
}

async fn assert_ambiguous_snapshot_withdraws(ambiguity: Ambiguity) {
        let ca = TestCa::new();
        let other = TestCa::new();
        let identity = ca.issue(LOCAL_ID, Duration::from_secs(600));
        let fixture = WorkloadApiFixture::start(vec![identity.clone()]).await;
        let audit = Arc::new(Recorder::default());
        let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit.clone());
        store.replace_registration(registration(&ca)).unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(driver.run(shutdown.clone()));
        let lease = material(&store).await;
        let before = store.source_revision();
        match ambiguity {
            Ambiguity::FederatedLocal => fixture.replace_with_federated_bundles(vec![identity], BTreeMap::from([("spiffe://example.org".into(), ca.root().to_vec())])),
            Ambiguity::PerSvid => fixture.replace(vec![other.issue(PEER_ID, Duration::from_secs(600)), identity]),
        }
        until(|| store.source_revision() > before).await;
        assert!(store.material().is_none(), "a complete snapshot must have one unambiguous bundle per trust domain");
        assert!(!lease.is_current());
        assert_eq!(audit.0.lock().unwrap().len(), 1);
        shutdown.cancel();
        task.await.unwrap();
        fixture.stop().await;
}

#[tokio::test]
async fn multiple_identities_share_an_equivalent_bundle_without_implicit_foreign_trust() {
    let ca = TestCa::new();
    let other = TestCa::new();
    let foreign = TestCa::new();
    let identity = ca.issue(LOCAL_ID, Duration::from_secs(600)).with_bundle(&[ca.root(), other.root()]);
    let peer = ca.issue(PEER_ID, Duration::from_secs(600)).with_bundle(&[other.root(), ca.root()]);
    let fixture = WorkloadApiFixture::start(vec![identity, peer]).await;
    fixture.replace_with_federated_bundles(vec![
        ca.issue(LOCAL_ID, Duration::from_secs(600)).with_bundle(&[ca.root(), other.root()]),
        ca.issue(PEER_ID, Duration::from_secs(600)).with_bundle(&[other.root(), ca.root()]),
    ], BTreeMap::from([("spiffe://foreign.example".into(), foreign.root().to_vec())]));
    let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), Arc::new(Recorder::default()));
    let original = registration(&ca);
    store.replace_registration(RegistrationSet::new(revision(1), revision(1), revision(1), original.expires_at(), original.bindings().to_vec(), vec![ca.root_sha256(), other.root_sha256()]).unwrap()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    assert!(lease.client_config(&spiffe::SpiffeId::new(PEER_ID).unwrap()).is_ok());
    assert!(lease.client_config(&spiffe::SpiffeId::new("spiffe://foreign.example/workload").unwrap()).is_err());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
