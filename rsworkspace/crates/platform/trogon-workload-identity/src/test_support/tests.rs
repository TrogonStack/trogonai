use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rcgen::{CustomExtension, ExtendedKeyUsagePurpose};
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::{FromDer, X509Certificate};

use super::*;
use crate::constants::TEST_TIMEOUT;
use crate::tests::{LOCAL_ID, PEER_ID, Recorder, local, material, registration, revision, until};
use crate::{IdentityConfig, IdentityStore, MaterialError};

fn leaf_params(id: &str) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth, ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names = vec![SanType::URI(Ia5String::try_from(id).unwrap())];
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::seconds(600);
    params
}

#[tokio::test]
async fn the_selected_intermediate_limits_activation_and_unrelated_expired_root_does_not_poison_it() {
    let ca = TestCa::new();
    let mut expired_params = CertificateParams::default();
    expired_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    expired_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    expired_params.not_before = time::OffsetDateTime::now_utc() - time::Duration::seconds(600);
    expired_params.not_after = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
    let expired = TestCa(CertifiedIssuer::self_signed(expired_params, KeyPair::generate().unwrap()).unwrap());
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::seconds(60);
    let intermediate = CertifiedIssuer::signed_by(params, KeyPair::generate().unwrap(), &ca.0).unwrap();
    let key = KeyPair::generate().unwrap();
    let leaf = leaf_params(LOCAL_ID).signed_by(&key, &intermediate).unwrap();
    let identity = TestIdentity {
        id: LOCAL_ID.to_owned(),
        chain: [leaf.der().as_ref(), intermediate.der().as_ref()].concat(),
        key: key.serialize_der(),
        bundle: [ca.root().as_ref(), expired.root().as_ref()].concat(),
    };
    let fixture = WorkloadApiFixture::start(vec![identity]).await;
    let (store, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local(), revision(1)),
        Arc::new(Recorder::default()),
    );
    let baseline = registration(&ca);
    store
        .replace_registration(
            crate::RegistrationSet::new(
                revision(1),
                revision(1),
                revision(1),
                baseline.expires_at(),
                baseline.bindings().to_vec(),
                vec![ca.root_sha256(), expired.root_sha256()],
            )
            .unwrap(),
        )
        .unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = material(&store).await;
    let (_, certificate) = X509Certificate::from_der(intermediate.der().as_ref()).unwrap();
    let expected = crate::certificate_validation::timestamp(certificate.validity().not_after.timestamp()).unwrap();
    assert_eq!(lease.expires_at(), expected);
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn incoming_peer_profiles_fail_closed_before_returning_registered_authority() {
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
    let mut ca_leaf = leaf_params(PEER_ID);
    ca_leaf.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_leaf.key_usages.push(KeyUsagePurpose::KeyCertSign);
    let mut server_only = leaf_params(PEER_ID);
    server_only.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    for identity in [
        ca.issue_with_params(PEER_ID, ca_leaf),
        ca.issue_with_params(PEER_ID, server_only),
        ca.issue("spiffe://example.org", Duration::from_secs(600)),
    ] {
        assert!(lease.peer_evidence_from_certificates(&identity.certificates()).is_err());
    }
    let mut trailing = ca.issue(PEER_ID, Duration::from_secs(600)).certificates()[0].to_vec();
    trailing.push(0xff);
    assert!(matches!(
        lease.peer_evidence_from_certificates(&[CertificateDer::from(trailing)]),
        Err(MaterialError::Profile)
    ));
    assert!(lease.peer_evidence_from_certificates(&[]).is_err());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn oversized_cryptographic_material_and_unbounded_root_sets_are_rejected_before_audit() {
    let ca = TestCa::new();
    let mut oversized_params = leaf_params(LOCAL_ID);
    oversized_params
        .custom_extensions
        .push(CustomExtension::from_oid_content(
            &[1, 2, 3, 4],
            vec![0; crate::constants::MAX_CONTEXT_BYTES],
        ));
    let oversized = ca.issue_with_params(LOCAL_ID, oversized_params);
    let roots: Vec<_> = (0..=crate::constants::MAX_ROOTS)
        .map(|_| TestCa::new().root())
        .collect();
    let too_many_roots = ca.issue(LOCAL_ID, Duration::from_secs(600)).with_bundle(&roots);
    for identity in [oversized, too_many_roots] {
        let fixture = WorkloadApiFixture::start(vec![identity]).await;
        let audit = Arc::new(Recorder::default());
        let (store, driver) = IdentityStore::new(IdentityConfig::new(fixture.endpoint(), local(), revision(1)), audit);
        store.replace_registration(registration(&ca)).unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(driver.run(shutdown.clone()));
        until(|| store.source_revision() > 0 && fixture.requests() > 0).await;
        assert!(store.material().is_none());
        shutdown.cancel();
        task.await.unwrap();
        fixture.stop().await;
    }
}

#[tokio::test]
async fn wire_fixture_refuses_missing_security_header_and_unknown_rpc() {
    let ca = TestCa::default();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    assert!(fixture.socket_exists());
    let channel = spiffe::transport::connect(&fixture.endpoint().endpoint())
        .await
        .unwrap();
    let mut grpc = tonic::client::Grpc::new(channel.clone());
    grpc.ready().await.unwrap();
    let response = grpc
        .server_streaming(
            Request::new(WireRequest {}),
            http::uri::PathAndQuery::from_static("/SpiffeWorkloadAPI/FetchX509SVID"),
            tonic_prost::ProstCodec::<WireRequest, WireResponse>::default(),
        )
        .await;
    assert_eq!(response.err().unwrap().code(), tonic::Code::InvalidArgument);
    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.unwrap();
    let mut request = Request::new(WireRequest {});
    request
        .metadata_mut()
        .insert("workload.spiffe.io", "true".parse().unwrap());
    let response = grpc
        .server_streaming(
            request,
            http::uri::PathAndQuery::from_static("/SpiffeWorkloadAPI/Unsupported"),
            tonic_prost::ProstCodec::<WireRequest, WireResponse>::default(),
        )
        .await;
    assert_eq!(response.err().unwrap().code(), tonic::Code::Unimplemented);
    fixture.stop().await;
}

#[tokio::test]
async fn fixture_can_close_a_complete_stream_and_finish_its_graceful_server_task() {
    let ca = TestCa::new();
    let fixture = WorkloadApiFixture::start(vec![ca.issue(LOCAL_ID, Duration::from_secs(600))]).await;
    let client = spiffe::WorkloadApiClient::connect(fixture.endpoint().endpoint())
        .await
        .unwrap();
    let mut stream = client.stream_x509_contexts().await.unwrap();
    assert!(
        tokio::time::timeout(TEST_TIMEOUT, stream.next())
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    let WorkloadApiFixture {
        updates,
        shutdown,
        task,
        directory,
        ..
    } = fixture;
    drop(updates);
    shutdown.cancel();
    assert!(
        tokio::time::timeout(TEST_TIMEOUT, stream.next())
            .await
            .unwrap()
            .is_none()
    );
    drop(stream);
    drop(client);
    tokio::time::timeout(TEST_TIMEOUT, task).await.unwrap().unwrap();
    drop(directory);
}
