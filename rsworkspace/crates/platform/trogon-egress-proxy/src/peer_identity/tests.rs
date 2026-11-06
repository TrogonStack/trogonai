use std::num::NonZeroU64;
use std::sync::Arc;

use rcgen::{ExtendedKeyUsagePurpose, IsCa, KeyUsagePurpose};
use tokio_util::sync::CancellationToken;
use trogon_workload_identity::test_support::{TestCa, WorkloadApiFixture};
use trogon_workload_identity::{IdentityConfig, IdentityStore, RegistrationBinding, RegistrationSet, WorkloadRole};

use rcgen::{CertificateParams, KeyPair, SanType, string::Ia5String};

use super::*;

struct ActivationAcknowledged;

#[async_trait::async_trait]
impl trogon_workload_identity::ActivationAudit for ActivationAcknowledged {
    async fn record(
        &self,
        _: &trogon_workload_identity::ActivationFact,
    ) -> Result<(), trogon_workload_identity::ActivationAuditError> {
        Ok(())
    }
}

#[tokio::test]
async fn rollback_after_peer_path_verification_cannot_restart_its_lifetime() {
    let wall = SystemTime::now();
    let local_id = "spiffe://trogon.example/service/egress";
    let peer_id = "spiffe://trogon.example/runtime/peer";
    let parameters = |id: &str, lifetime: Duration| {
        let mut parameters = CertificateParams::default();
        parameters.subject_alt_names = vec![SanType::URI(Ia5String::try_from(id).expect("URI"))];
        parameters.is_ca = IsCa::ExplicitNoCa;
        parameters.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        parameters.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
        parameters.not_before = (wall - Duration::from_secs(120)).into();
        parameters.not_after = (wall + lifetime).into();
        parameters
    };
    let ca = TestCa::new();
    let local = ca.issue_with_params(local_id, parameters(local_id, Duration::from_secs(120)));
    let peer = ca.issue_with_params(peer_id, parameters(peer_id, Duration::from_secs(10)));
    let fixture = WorkloadApiFixture::start(vec![local]).await;
    let local = RegistrationBinding::new(local_id, "acme", "payments", WorkloadRole::EgressProxy, None).expect("local");
    let registered_peer =
        RegistrationBinding::new(peer_id, "acme", "payments", WorkloadRole::Agent, Some("agent-1")).expect("peer");
    let (identity, driver) = IdentityStore::new(
        IdentityConfig::new(fixture.endpoint(), local.clone(), NonZeroU64::MIN),
        Arc::new(ActivationAcknowledged),
    );
    identity
        .replace_registration(
            RegistrationSet::new(
                NonZeroU64::MIN,
                NonZeroU64::MIN,
                NonZeroU64::MIN,
                wall + Duration::from_secs(120),
                vec![local, registered_peer],
                vec![ca.root_sha256()],
            )
            .expect("registration"),
        )
        .expect("governed authority");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    let lease = tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, async {
        loop {
            if let Some(lease) = identity.material() {
                return lease;
            }
            tokio::time::sleep(crate::constants::LIVE_POLICY_TEST_POLL).await;
        }
    })
    .await
    .expect("approved material");
    let began = Instant::now();
    let observed = PeerIdentity::from_workload_certificates_with_clock(&peer.certificates(), lease.clone(), || {
        wall - Duration::from_secs(30)
    });
    let inspected = observed.as_ref().expect("authenticated peer");
    let debug = format!("{inspected:?}");
    assert!(debug.contains("agent-1"));
    assert!(debug.contains(peer_id));
    assert!(!debug.contains("PRIVATE KEY"));
    assert!(!debug.contains("key_der"));
    let withdrawn = PeerIdentity::from_workload_certificates_with_clock(&peer.certificates(), lease, || {
        identity.withdraw_registration();
        wall
    });
    assert!(matches!(
        withdrawn,
        Err(WorkloadPeerIdentityError::Material(MaterialError::Unavailable))
    ));
    shutdown.cancel();
    task.await.expect("identity driver");
    fixture.stop().await;
    let observed = observed.expect("authenticated peer");
    assert!(
        observed.deadline() <= began + Duration::from_secs(10),
        "a second wall sample cannot extend the path verified by the first sample"
    );
}

#[test]
fn a_delayed_wall_clock_read_consumes_the_peer_path_lifetime() {
    let began = std::cell::Cell::new(None);
    let wall = SystemTime::now();
    let lifetime = Duration::from_secs(60);
    let peer = PeerIdentity::with_clock(AgentId::new("agent-1").expect("agent"), wall + lifetime, || {
        began.set(Some(Instant::now()));
        std::thread::sleep(Duration::from_millis(20));
        wall
    })
    .expect("identity");
    assert!(
        peer.deadline() <= began.get().expect("clock sampled") + lifetime,
        "the monotonic deadline must include clock-read delay"
    );
}

fn certificate(lifetime: Duration) -> CertificateDer<'static> {
    let mut params = CertificateParams::default();
    params.subject_alt_names = vec![SanType::URI(
        Ia5String::try_from("spiffe://trogon.example/agent/agent-1").expect("uri"),
    )];
    params.not_after = (SystemTime::now() + lifetime).into();
    params
        .self_signed(&KeyPair::generate().expect("key"))
        .expect("certificate")
        .der()
        .clone()
}

#[test]
fn the_shortest_presented_certificate_bounds_the_identity() {
    let leaf = certificate(Duration::from_secs(3600));
    let issuer = certificate(Duration::from_secs(60));
    let (_, parsed) = X509Certificate::from_der(issuer.as_ref()).expect("issuer");
    let expected = SystemTime::UNIX_EPOCH
        + Duration::from_secs(parsed.validity().not_after.timestamp().try_into().expect("timestamp"));
    let peer =
        PeerIdentity::from_verified_certificates(&[leaf, issuer], &TrustDomain::new("trogon.example").expect("domain"))
            .expect("identity");
    assert_eq!(peer.expires_at(), expected);
    assert_eq!(peer.agent().as_str(), "agent-1");
    assert!(peer.deadline() - Instant::now() <= Duration::from_secs(60));
}

#[test]
fn a_long_certificate_cannot_extend_the_session_backstop() {
    let now = Instant::now();
    let peer = PeerIdentity::from_verified_certificates(
        &[certificate(Duration::from_secs(3600))],
        &TrustDomain::new("trogon.example").expect("domain"),
    )
    .expect("identity");
    assert!(format!("{peer:?}").contains("workload_peer: None"));
    assert!(peer.deadline() >= now + MAX_SESSION_LIFETIME);
    assert!(peer.deadline() <= Instant::now() + MAX_SESSION_LIFETIME);
}

#[test]
fn expiry_is_exclusive_and_an_expired_chain_cannot_start_a_session() {
    let agent = AgentId::new("agent-1").expect("agent");
    let now = SystemTime::now();
    assert!(matches!(
        PeerIdentity::at(agent.clone(), now, now),
        Err(PeerIdentityError::Validity)
    ));
    assert!(matches!(
        PeerIdentity::at(agent, now - Duration::from_secs(1), now),
        Err(PeerIdentityError::Validity)
    ));
}

#[test]
fn trailing_der_and_empty_chains_are_refused() {
    let domain = TrustDomain::new("trogon.example").expect("domain");
    let mut der = certificate(Duration::from_secs(60)).as_ref().to_vec();
    der.push(0);
    assert!(matches!(
        PeerIdentity::from_verified_certificates(&[CertificateDer::from(der)], &domain),
        Err(PeerIdentityError::Unparsable)
    ));
    assert!(matches!(
        PeerIdentity::from_verified_certificates(&[], &domain),
        Err(PeerIdentityError::NoUriSan)
    ));
}

#[test]
fn malformed_or_trailing_der_in_a_presented_intermediate_is_refused() {
    let domain = TrustDomain::new("trogon.example").expect("domain");
    let leaf = certificate(Duration::from_secs(60));
    let mut trailing = certificate(Duration::from_secs(60)).as_ref().to_vec();
    trailing.push(0);
    for malformed in [vec![0], trailing] {
        assert!(matches!(
            PeerIdentity::from_verified_certificates(&[leaf.clone(), CertificateDer::from(malformed)], &domain),
            Err(PeerIdentityError::Unparsable)
        ));
    }
}
