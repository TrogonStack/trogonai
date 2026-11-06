use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use futures::StreamExt;
use rustls::pki_types::ServerName;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::constants::TEST_TIMEOUT;
use crate::test_support::{TestCa, WorkloadApiFixture};
use crate::*;

pub(crate) const LOCAL_ID: &str = "spiffe://example.org/service/egress";
pub(crate) const PEER_ID: &str = "spiffe://example.org/runtime/workload-1";

mod bundle_snapshot_tests;
mod crl_tests;
mod lifecycle_tests;
mod spiffe_id_bounds_tests;
mod trust_revocation_tests;
mod validation_tests;
mod wire_identity_tests;

#[derive(Default)]
pub(crate) struct Recorder(Mutex<Vec<ActivationFact>>);

#[async_trait]
impl ActivationAudit for Recorder {
    async fn record(&self, fact: &ActivationFact) -> Result<(), ActivationAuditError> {
        self.0.lock().unwrap().push(fact.clone());
        Ok(())
    }
}

pub(crate) fn local() -> RegistrationBinding {
    RegistrationBinding::new(LOCAL_ID, "tenant", "project", WorkloadRole::EgressProxy, None).unwrap()
}
pub(crate) fn peer() -> RegistrationBinding {
    RegistrationBinding::new(PEER_ID, "tenant", "project", WorkloadRole::Agent, Some("agent-1")).unwrap()
}
pub(crate) fn revision(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}
pub(crate) fn registration(ca: &TestCa) -> RegistrationSet {
    RegistrationSet::new(
        revision(1),
        revision(1),
        revision(1),
        SystemTime::now() + Duration::from_secs(300),
        vec![local(), peer()],
        vec![ca.root_sha256()],
    )
    .unwrap()
}

pub(crate) async fn material(store: &IdentityStore) -> MaterialLease {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(material) = store.material() {
                return material;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("verified material must activate")
}

#[tokio::test]
async fn supported_sdk_reads_real_unix_workload_stream() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let client = spiffe::WorkloadApiClient::connect(fixture.endpoint().endpoint())
        .await
        .unwrap();
    let mut stream = client.stream_x509_contexts().await.unwrap();
    let context = tokio::time::timeout(TEST_TIMEOUT, stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(context.svids()[0].spiffe_id().to_string(), LOCAL_ID);
    assert_eq!(fixture.requests(), 1);
    fixture.permission_denied();
    assert!(matches!(
        tokio::time::timeout(TEST_TIMEOUT, stream.next())
            .await
            .unwrap()
            .unwrap(),
        Err(spiffe::WorkloadApiError::PermissionDenied(_))
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn verified_material_activates_only_with_exact_registration_and_durable_record() {
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
    assert!(lease.is_current());
    assert_eq!(
        lease
            .application_for_peer(&spiffe::SpiffeId::new(PEER_ID).unwrap())
            .unwrap()
            .as_str(),
        "agent-1"
    );
    assert_eq!(audit.0.lock().unwrap().len(), 1);
    let recorded = audit.0.lock().unwrap()[0].clone();
    assert_eq!(recorded.binding(), store.config().local());
    assert_eq!(recorded.recovery_fence(), store.config().recovery_fence());
    assert_eq!(recorded.trust_revision(), lease.registration().trust_revision());
    assert!(recorded.not_before() <= SystemTime::now());
    assert_eq!(recorded.expires_at(), lease.expires_at());
    assert_eq!(
        *recorded.leaf_sha256(),
        <[u8; 32]>::from(Sha256::digest(&identity.certificates()[0]))
    );
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let key = provider.key_provider.load_private_key(identity.private_key()).unwrap();
    assert_eq!(
        *recorded.public_key_sha256(),
        <[u8; 32]>::from(Sha256::digest(key.public_key().unwrap()))
    );
    assert_eq!(
        *recorded.bundle_sha256(),
        <[u8; 32]>::from(Sha256::digest(ca.root_sha256()))
    );
    assert_eq!(store.config().endpoint().as_uri(), fixture.endpoint().as_uri());
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
        self.permits
            .acquire()
            .await
            .map_err(|_| ActivationAuditError::Unavailable)?
            .forget();
        Ok(())
    }
}

pub(crate) async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("state transition");
}

#[tokio::test]
async fn permission_withdrawal_is_observed_while_activation_audit_is_pending() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let audit = Arc::new(HeldAudit {
        entered: AtomicUsize::new(0),
        permits: Semaphore::new(0),
    });
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
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
    let fixture = WorkloadApiFixture::start(vec![
        ca.issue(PEER_ID, Duration::from_secs(600)),
        local_identity.clone(),
    ])
    .await;
    let audit = Arc::new(Recorder::default());
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
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
        valid
            .clone()
            .with_private_key(&ca.issue(LOCAL_ID, Duration::from_secs(600))),
        wrong_ca.issue(LOCAL_ID, Duration::from_secs(600)),
        ca.issue(LOCAL_ID, Duration::from_secs(7200)),
        valid.with_bundle(&[]),
    ] {
        let fixture = WorkloadApiFixture::start(vec![identity]).await;
        let audit = Arc::new(Recorder::default());
        let (store, driver) = IdentityStore::new(
            IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
            audit.clone(),
        );
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

pub(crate) fn handshake(
    server: Arc<rustls::ServerConfig>,
    client: Arc<rustls::ClientConfig>,
) -> Result<(), rustls::Error> {
    let mut server = rustls::ServerConnection::new(server)?;
    let mut client = rustls::ClientConnection::new(client, ServerName::try_from("unrelated.example").unwrap())?;
    for _ in 0..16 {
        let mut flight = Vec::new();
        client.write_tls(&mut flight).unwrap();
        if !flight.is_empty() {
            server.read_tls(&mut std::io::Cursor::new(flight)).unwrap();
            server.process_new_packets()?;
        }
        let mut flight = Vec::new();
        server.write_tls(&mut flight).unwrap();
        if !flight.is_empty() {
            client.read_tls(&mut std::io::Cursor::new(flight)).unwrap();
            client.process_new_packets()?;
        }
        if !client.is_handshaking() && !server.is_handshaking() {
            return Ok(());
        }
    }
    panic!("TLS handshake did not settle")
}

#[tokio::test]
async fn real_mutual_tls_authenticates_exact_registered_spiffe_ids_without_dns_names() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![
        ca.issue(LOCAL_ID, Duration::from_secs(600)),
        ca.issue(PEER_ID, Duration::from_secs(600)),
    ])
    .await;
    let (server, server_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let (client, client_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), peer(), revision(1)),
        Arc::new(Recorder::default()),
    );
    server.replace_registration(registration(&ca)).unwrap();
    client.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let server_task = tokio::spawn(server_driver.run(shutdown.clone()));
    let client_task = tokio::spawn(client_driver.run(shutdown.clone()));
    let server_lease = material(&server).await;
    let client_lease = material(&client).await;
    let local_id = spiffe::SpiffeId::new(LOCAL_ID).unwrap();
    assert!(
        handshake(
            server_lease.server_config(),
            client_lease.client_config(&local_id).unwrap()
        )
        .is_ok()
    );
    assert!(
        handshake(
            server_lease.server_config(),
            client_lease
                .client_config(&spiffe::SpiffeId::new(PEER_ID).unwrap())
                .unwrap()
        )
        .is_err()
    );
    assert!(
        client_lease
            .client_config(&spiffe::SpiffeId::new("spiffe://example.org/unregistered").unwrap())
            .is_err()
    );
    let rogue = ca.issue("spiffe://example.org/unregistered", Duration::from_secs(600));
    let mut snapshot = registration(&ca);
    let mut bindings = snapshot.bindings().to_vec();
    bindings
        .push(RegistrationBinding::new(rogue.id(), "tenant", "project", WorkloadRole::Agent, Some("rogue")).unwrap());
    snapshot = RegistrationSet::new(
        revision(1),
        revision(2),
        revision(1),
        snapshot.expires_at(),
        bindings,
        snapshot.approved_roots().to_vec(),
    )
    .unwrap();
    fixture.replace(vec![ca.issue(LOCAL_ID, Duration::from_secs(600)), rogue]);
    let rogue_binding = snapshot
        .bindings()
        .iter()
        .find(|binding| binding.application_id().is_some_and(|id| id.as_str() == "rogue"))
        .unwrap()
        .clone();
    let (rogue_store, rogue_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), rogue_binding, revision(1)),
        Arc::new(Recorder::default()),
    );
    rogue_store.replace_registration(snapshot).unwrap();
    let rogue_task = tokio::spawn(rogue_driver.run(shutdown.clone()));
    let rogue_lease = material(&rogue_store).await;
    until(|| {
        server
            .material()
            .is_some_and(|lease| lease.fact().source_revision() > server_lease.fact().source_revision())
    })
    .await;
    let current_server = material(&server).await;
    assert!(current_server.is_current());
    assert!(
        handshake(
            current_server.server_config(),
            rogue_lease.client_config(&local_id).unwrap()
        )
        .is_err()
    );
    shutdown.cancel();
    server_task.await.unwrap();
    client_task.await.unwrap();
    rogue_task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn revoked_registration_cannot_replay_and_watch_catches_a_prior_withdrawal() {
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
    store.withdraw_registration();
    tokio::time::timeout(TEST_TIMEOUT, lease.changed())
        .await
        .expect("withdrawal before watch is retained");
    assert_eq!(
        store.replace_registration(original.clone()),
        Err(RegistrationError::Revision)
    );
    let renewed = RegistrationSet::new(
        revision(1),
        revision(2),
        revision(1),
        original.expires_at(),
        original.bindings().to_vec(),
        original.approved_roots().to_vec(),
    )
    .unwrap();
    store.replace_registration(renewed).unwrap();
    let fresh = material(&store).await;
    assert!(fresh.fact().activation_revision() > lease.fact().activation_revision());
    assert!(!lease.is_current());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn changed_registration_cancels_pending_activation_and_requires_new_audit() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let audit = Arc::new(HeldAudit {
        entered: AtomicUsize::new(0),
        permits: Semaphore::new(0),
    });
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        audit.clone(),
    );
    let original = registration(&ca);
    store.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    until(|| audit.entered.load(Ordering::SeqCst) == 1).await;
    let renewed = RegistrationSet::new(
        revision(1),
        revision(2),
        revision(1),
        original.expires_at(),
        original.bindings().to_vec(),
        original.approved_roots().to_vec(),
    )
    .unwrap();
    store.replace_registration(renewed).unwrap();
    until(|| audit.entered.load(Ordering::SeqCst) == 2).await;
    assert!(store.material().is_none());
    audit.permits.add_permits(1);
    let lease = material(&store).await;
    assert_eq!(lease.fact().registration_revision(), revision(2));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn cached_tls_configurations_cannot_authenticate_after_credential_withdrawal() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![
        ca.issue(LOCAL_ID, Duration::from_secs(600)),
        ca.issue(PEER_ID, Duration::from_secs(600)),
    ])
    .await;
    let (server, server_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let (client, client_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), peer(), revision(1)),
        Arc::new(Recorder::default()),
    );
    server.replace_registration(registration(&ca)).unwrap();
    let original = registration(&ca);
    client.replace_registration(original.clone()).unwrap();
    let shutdown = CancellationToken::new();
    let server_task = tokio::spawn(server_driver.run(shutdown.clone()));
    let client_task = tokio::spawn(client_driver.run(shutdown.clone()));
    let server_config = material(&server).await.server_config();
    let client_config = material(&client)
        .await
        .client_config(&spiffe::SpiffeId::new(LOCAL_ID).unwrap())
        .unwrap();
    let resolved_key = client_config
        .client_auth_cert_resolver
        .resolve(&[], &[rustls::SignatureScheme::ECDSA_NISTP256_SHA256])
        .expect("active client key");
    assert!(!format!("{:?}", client_config.client_auth_cert_resolver).contains("BEGIN PRIVATE KEY"));
    resolved_key
        .keys_match()
        .expect("live signing wrapper preserves exact certificate and key matching");
    let signer = resolved_key
        .key
        .choose_scheme(&[rustls::SignatureScheme::ECDSA_NISTP256_SHA256])
        .expect("active signer");
    assert!(handshake(Arc::clone(&server_config), Arc::clone(&client_config)).is_ok());
    client.withdraw_registration();
    assert!(signer.sign(b"withdrawn credential proof").is_err());
    assert!(
        handshake(Arc::clone(&server_config), client_config).is_err(),
        "cached client configuration must not reuse withdrawn private-key authority"
    );
    client
        .replace_registration(
            RegistrationSet::new(
                revision(1),
                revision(2),
                revision(1),
                original.expires_at(),
                original.bindings().to_vec(),
                original.approved_roots().to_vec(),
            )
            .unwrap(),
        )
        .unwrap();
    let client_config = material(&client)
        .await
        .client_config(&spiffe::SpiffeId::new(LOCAL_ID).unwrap())
        .unwrap();
    assert!(handshake(Arc::clone(&server_config), Arc::clone(&client_config)).is_ok());
    server.withdraw_registration();
    assert!(
        handshake(server_config, client_config).is_err(),
        "cached server configuration must not reuse withdrawn private-key authority"
    );
    shutdown.cancel();
    server_task.await.unwrap();
    client_task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn cached_server_configuration_refuses_handshakes_after_registration_lease_expiry() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![
        ca.issue(LOCAL_ID, Duration::from_secs(600)),
        ca.issue(PEER_ID, Duration::from_secs(600)),
    ])
    .await;
    let (server, server_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let (client, client_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), peer(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let original = registration(&ca);
    server
        .replace_registration(
            RegistrationSet::new(
                revision(1),
                revision(1),
                revision(1),
                SystemTime::now() + Duration::from_secs(2),
                original.bindings().to_vec(),
                original.approved_roots().to_vec(),
            )
            .unwrap(),
        )
        .unwrap();
    client.replace_registration(original).unwrap();
    let shutdown = CancellationToken::new();
    let server_task = tokio::spawn(server_driver.run(shutdown.clone()));
    let client_task = tokio::spawn(client_driver.run(shutdown.clone()));
    let server_lease = material(&server).await;
    let server_config = server_lease.server_config();
    let client_config = material(&client)
        .await
        .client_config(&spiffe::SpiffeId::new(LOCAL_ID).unwrap())
        .unwrap();
    assert!(handshake(Arc::clone(&server_config), Arc::clone(&client_config)).is_ok());
    tokio::time::sleep_until(server_lease.deadline()).await;
    assert!(handshake(server_config, client_config).is_err());
    assert!(!server_lease.is_current());
    shutdown.cancel();
    server_task.await.unwrap();
    client_task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn peer_evidence_is_bounded_by_its_selected_root_instead_of_the_local_svid_path() {
    let local_ca = TestCa::new();
    let peer_ca = TestCa::with_lifetime(Duration::from_secs(60));
    let roots = [local_ca.root(), peer_ca.root()];
    let peer_identity = peer_ca.issue(PEER_ID, Duration::from_secs(600)).with_bundle(&roots);
    let fixture = WorkloadApiFixture::start(vec![
        local_ca.issue(LOCAL_ID, Duration::from_secs(600)).with_bundle(&roots),
        peer_identity.clone(),
    ])
    .await;
    let (server, server_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let (client, client_driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), peer(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let registration = RegistrationSet::new(
        revision(1),
        revision(1),
        revision(1),
        SystemTime::now() + Duration::from_secs(300),
        vec![local(), peer()],
        vec![local_ca.root_sha256(), peer_ca.root_sha256()],
    )
    .unwrap();
    server.replace_registration(registration.clone()).unwrap();
    client.replace_registration(registration).unwrap();
    let shutdown = CancellationToken::new();
    let server_task = tokio::spawn(server_driver.run(shutdown.clone()));
    let client_task = tokio::spawn(client_driver.run(shutdown.clone()));
    let server_lease = material(&server).await;
    let client_lease = material(&client).await;
    assert!(
        handshake(
            server_lease.server_config(),
            client_lease
                .client_config(&spiffe::SpiffeId::new(LOCAL_ID).unwrap())
                .unwrap()
        )
        .is_ok()
    );
    let evidence = server_lease
        .peer_evidence_from_certificates(&peer_identity.certificates())
        .unwrap();
    assert_eq!(
        server_lease
            .peer_identity_from_certificates(&peer_identity.certificates())
            .unwrap(),
        *evidence.application_id()
    );
    assert_eq!(evidence.application_id().as_str(), "agent-1");
    assert_eq!(evidence.spiffe_id().to_string(), PEER_ID);
    assert_eq!(
        *evidence.leaf_sha256(),
        <[u8; 32]>::from(Sha256::digest(&peer_identity.certificates()[0]))
    );
    let root = peer_ca.root();
    let (_, root) = X509Certificate::from_der(root.as_ref()).unwrap();
    let root_expiry = crate::certificate_validation::timestamp(root.validity().not_after.timestamp()).unwrap();
    assert_eq!(
        evidence.expires_at(),
        root_expiry,
        "a peer must not inherit the server's longer-lived issuing path"
    );
    assert!(evidence.expires_at() < server_lease.expires_at());
    shutdown.cancel();
    server_task.await.unwrap();
    client_task.await.unwrap();
    fixture.stop().await;
}
