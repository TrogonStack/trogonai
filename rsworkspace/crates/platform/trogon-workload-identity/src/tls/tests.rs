use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::material_authority::{MaterialAuthority, MaterialStatus};
use crate::test_support::{TestCa, WorkloadApiFixture};
use crate::tests::{LOCAL_ID, PEER_ID, Recorder, handshake, local, material, peer, registration, revision};
use crate::{IdentityConfig, IdentityStore, MaterialLease};

struct LeaseStatus(MaterialLease);

impl std::fmt::Debug for LeaseStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseStatus").finish_non_exhaustive()
    }
}

impl MaterialStatus for LeaseStatus {
    fn is_current(&self) -> bool {
        self.0.is_current()
    }
}

#[tokio::test]
async fn tls12_uses_registered_spiffe_verification_and_live_signing_authority() {
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
    let expected = spiffe::SpiffeId::new(LOCAL_ID).unwrap();
    let original = client_lease.client_config(&expected).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = RegisteredServerVerifier {
        roots: vec![ca.root()],
        expected,
        algorithms: provider.signature_verification_algorithms,
        authority: MaterialAuthority::new(Arc::new(LeaseStatus(client_lease.clone()))),
    };
    let client_config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS12])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_cert_resolver(Arc::clone(&original.client_auth_cert_resolver));
    let client_config = Arc::new(client_config);
    let server_config = server_lease.server_config();
    assert!(handshake(Arc::clone(&server_config), Arc::clone(&client_config)).is_ok());
    client.withdraw_registration();
    assert!(handshake(server_config, client_config).is_err());
    shutdown.cancel();
    server_task.await.unwrap();
    client_task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn client_verification_cannot_promote_a_downstream_pkix_denial() {
    let ca = TestCa::new();
    let stricter_ca = TestCa::new();
    let identity = ca.issue(PEER_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let mut roots = RootCertStore::empty();
    roots.add(stricter_ca.root()).unwrap();
    let inner = WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
    )
    .build()
    .unwrap();
    let certificates = identity.certificates();
    let now = UnixTime::now();
    let expected = inner
        .verify_client_cert(&certificates[0], &[], now)
        .expect_err("stricter trust policy must reject this signing chain");
    assert!(matches!(&expected, rustls::Error::InvalidCertificate(_)));
    let verifier = RegisteredClientVerifier {
        inner,
        roots: vec![ca.root()],
        registration: registration(&ca),
        authority: MaterialAuthority::new(Arc::new(LeaseStatus(lease.clone()))),
    };
    assert_eq!(
        verifier.verify_client_cert(&certificates[0], &[], now).unwrap_err(),
        expected
    );
    assert!(lease.is_current());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
