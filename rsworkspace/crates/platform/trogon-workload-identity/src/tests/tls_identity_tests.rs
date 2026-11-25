use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose, SanType};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use std::collections::BTreeMap;

use super::*;
use crate::test_support::TestIdentity;

const SECOND_ID: &str = "spiffe://example.org/runtime/workload-2";

#[derive(Debug)]
struct AcceptAnyServer(rustls::crypto::WebPkiSupportedAlgorithms);

impl ServerCertVerifier for AcceptAnyServer {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// A client that presents `identity` and trusts whatever server answers, so the
/// only verdict under test is the listener's.
fn presenting(identity: &TestIdentity) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = AcceptAnyServer(provider.signature_verification_algorithms);
    Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_client_auth_cert(identity.certificates(), identity.private_key())
            .unwrap(),
    )
}

fn leaf_naming(uris: &[&str]) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.is_ca = IsCa::ExplicitNoCa;
    params.subject_alt_names = uris
        .iter()
        .map(|uri| SanType::URI(rcgen::string::Ia5String::try_from(*uri).unwrap()))
        .collect();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::seconds(1);
    params.not_after = now + time::Duration::seconds(600);
    params
}

fn refused_as_application_failure(result: &Result<(), rustls::Error>) -> bool {
    matches!(
        result,
        Err(rustls::Error::InvalidCertificate(
            CertificateError::ApplicationVerificationFailure
        ))
    )
}

#[tokio::test]
async fn a_client_leaf_with_two_uri_sans_is_refused_by_the_listener() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;

    let single = ca.issue_with_params(PEER_ID, leaf_naming(&[PEER_ID]));
    assert!(handshake(lease.server_config(), presenting(&single)).is_ok());

    let double = ca.issue_with_params(PEER_ID, leaf_naming(&[PEER_ID, SECOND_ID]));
    assert!(refused_as_application_failure(&handshake(
        lease.server_config(),
        presenting(&double)
    )));

    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn a_federated_bundle_never_verifies_a_leaf_claiming_the_local_trust_domain() {
    let ca = TestCa::new();
    let foreign = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    fixture.replace_with_federated_bundles(
        vec![ca.issue(LOCAL_ID, Duration::from_secs(600))],
        BTreeMap::from([("spiffe://foreign.example".into(), foreign.root().to_vec())]),
    );
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;

    let genuine = ca.issue_with_params(PEER_ID, leaf_naming(&[PEER_ID]));
    assert!(handshake(lease.server_config(), presenting(&genuine)).is_ok());

    let forged = foreign.issue_with_params(PEER_ID, leaf_naming(&[PEER_ID]));
    assert!(handshake(lease.server_config(), presenting(&forged)).is_err());

    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn a_client_refuses_a_server_that_presents_a_different_spiffe_id_than_expected() {
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

    let expected_but_absent = spiffe::SpiffeId::new(PEER_ID).unwrap();
    let outcome = handshake(
        server_lease.server_config(),
        client_lease.client_config(&expected_but_absent).unwrap(),
    );
    assert!(refused_as_application_failure(&outcome), "{outcome:?}");

    shutdown.cancel();
    server_task.await.unwrap();
    client_task.await.unwrap();
    fixture.stop().await;
}

/// Presents `certificates` but signs the handshake with `signer`, skipping the
/// consistency check `with_client_auth_cert` applies, so the listener alone
/// decides whether the sender holds the certificate's private key.
#[derive(Debug)]
struct SignsWith(Arc<rustls::sign::CertifiedKey>);

impl rustls::client::ResolvesClientCert for SignsWith {
    fn resolve(&self, _: &[&[u8]], _: &[SignatureScheme]) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }

    fn has_certs(&self) -> bool {
        true
    }
}

fn presenting_certificate_of_signing_with_key_of(
    certificate: &TestIdentity,
    signer: &TestIdentity,
) -> Arc<ClientConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let key = provider.key_provider.load_private_key(signer.private_key()).unwrap();
    let verifier = AcceptAnyServer(provider.signature_verification_algorithms);
    Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_client_cert_resolver(Arc::new(SignsWith(Arc::new(rustls::sign::CertifiedKey::new(
                certificate.certificates(),
                key,
            ))))),
    )
}

#[tokio::test]
async fn a_peer_certificate_without_its_private_key_fails_the_handshake() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    store.replace_registration(registration(&ca)).unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;

    let registered = ca.issue_with_params(PEER_ID, leaf_naming(&[PEER_ID]));
    assert!(
        handshake(
            lease.server_config(),
            presenting_certificate_of_signing_with_key_of(&registered, &registered)
        )
        .is_ok()
    );

    let thief = ca.issue_with_params(SECOND_ID, leaf_naming(&[SECOND_ID]));
    let outcome = handshake(
        lease.server_config(),
        presenting_certificate_of_signing_with_key_of(&registered, &thief),
    );
    assert!(
        matches!(
            outcome,
            Err(rustls::Error::InvalidCertificate(CertificateError::BadSignature))
        ),
        "{outcome:?}"
    );

    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}
