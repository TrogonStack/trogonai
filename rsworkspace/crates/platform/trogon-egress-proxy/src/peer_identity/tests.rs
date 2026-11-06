use rcgen::{CertificateParams, KeyPair, SanType, string::Ia5String};

use super::*;

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
