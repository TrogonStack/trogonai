use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType, string::Ia5String,
};
use rustls::internal::msgs::codec::{Codec, Reader};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::{ClientConfig, ClientConnection, Connection, HandshakeKind, ProtocolVersion, ServerConnection};

use super::*;

fn domain() -> TrustDomain {
    TrustDomain::new("trogon.example").expect("trust domain")
}

fn ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    CertifiedIssuer::self_signed(params, KeyPair::generate().expect("key")).expect("ca")
}

/// Validity that sits inside the one-hour X.509-SVID ceiling.
fn one_hour_validity(params: &mut CertificateParams) {
    let now = std::time::SystemTime::now();
    params.not_before = (now - std::time::Duration::from_secs(60)).into();
    params.not_after = (now + std::time::Duration::from_secs(1800)).into();
}

/// A server certificate that satisfies the X.509-SVID profile and lifetime ceiling.
fn server_params() -> CertificateParams {
    let mut params = CertificateParams::new(vec!["proxy.example".to_owned()]).expect("server params");
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    one_hour_validity(&mut params);
    params
}

fn conforming_params(uris: &[&str]) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.is_ca = IsCa::ExplicitNoCa;
    params.subject_alt_names = uris
        .iter()
        .map(|uri| SanType::URI(Ia5String::try_from(*uri).expect("ia5")))
        .collect();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    one_hour_validity(&mut params);
    params
}

fn leaf_from(issuer: &CertifiedIssuer<'static, KeyPair>, params: CertificateParams) -> CertificateDer<'static> {
    let key = KeyPair::generate().expect("key");
    params.signed_by(&key, issuer).expect("leaf").der().clone()
}

fn client_leaf(issuer: &CertifiedIssuer<'static, KeyPair>, uris: &[&str]) -> CertificateDer<'static> {
    leaf_from(issuer, conforming_params(uris))
}

fn verifier(anchor: &CertifiedIssuer<'static, KeyPair>) -> SingleAgentVerifier {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = RootCertStore::empty();
    roots.add(anchor.der().clone()).expect("root");
    let inner = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
        .build()
        .expect("verifier");
    SingleAgentVerifier {
        inner,
        trust_domain: domain(),
    }
}

fn now() -> UnixTime {
    UnixTime::now()
}

#[test]
fn accepts_a_leaf_from_the_anchor_with_one_agent_uri() {
    let anchor = ca("anchor");
    let leaf = client_leaf(&anchor, &["spiffe://trogon.example/agent/agent-1"]);
    assert!(verifier(&anchor).verify_client_cert(&leaf, &[], now()).is_ok());
}

#[test]
fn refuses_a_leaf_from_another_ca() {
    let anchor = ca("anchor");
    let stranger = ca("stranger");
    let leaf = client_leaf(&stranger, &["spiffe://trogon.example/agent/agent-1"]);
    assert!(verifier(&anchor).verify_client_cert(&leaf, &[], now()).is_err());
}

#[test]
fn refuses_zero_two_and_foreign_uris() {
    let anchor = ca("anchor");
    let verifier = verifier(&anchor);
    for uris in [
        vec![],
        vec![
            "spiffe://trogon.example/agent/agent-1",
            "spiffe://trogon.example/agent/agent-2",
        ],
        vec!["spiffe://other.example/agent/agent-1"],
        vec!["https://trogon.example/agent/agent-1"],
    ] {
        let leaf = client_leaf(&anchor, &uris);
        assert!(
            verifier.verify_client_cert(&leaf, &[], now()).is_err(),
            "{uris:?} must be refused"
        );
    }
}

const AGENT_URI: &str = "spiffe://trogon.example/agent/agent-1";

fn violations() -> Vec<(&'static str, CertificateParams)> {
    let mut key_cert_sign = conforming_params(&[AGENT_URI]);
    key_cert_sign.key_usages.push(KeyUsagePurpose::KeyCertSign);
    let mut crl_sign = conforming_params(&[AGENT_URI]);
    crl_sign.key_usages.push(KeyUsagePurpose::CrlSign);
    let mut ca_leaf = conforming_params(&[AGENT_URI]);
    ca_leaf.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mut no_signature = conforming_params(&[AGENT_URI]);
    no_signature.key_usages = vec![KeyUsagePurpose::KeyEncipherment];
    let mut no_key_usage = conforming_params(&[AGENT_URI]);
    no_key_usage.key_usages = Vec::new();
    let mut client_only = conforming_params(&[AGENT_URI]);
    client_only.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let mut server_only = conforming_params(&[AGENT_URI]);
    server_only.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    vec![
        ("keyCertSign", key_cert_sign),
        ("cRLSign", crl_sign),
        ("cA=true", ca_leaf),
        ("no digitalSignature", no_signature),
        ("no keyUsage", no_key_usage),
        ("clientAuth only", client_only),
        ("serverAuth only", server_only),
    ]
}

#[test]
fn refuses_a_leaf_that_breaks_the_svid_key_constraints() {
    let anchor = ca("anchor");
    let verifier = verifier(&anchor);
    for (name, params) in violations() {
        let leaf = leaf_from(&anchor, params);
        assert!(
            verifier.verify_client_cert(&leaf, &[], now()).is_err(),
            "{name} must be refused"
        );
    }
}

#[test]
fn accepts_a_conforming_leaf_without_extended_key_usage() {
    let anchor = ca("anchor");
    let verifier = verifier(&anchor);
    let mut no_eku = conforming_params(&[AGENT_URI]);
    no_eku.extended_key_usages = Vec::new();
    let leaf = leaf_from(&anchor, no_eku);
    assert!(verifier.verify_client_cert(&leaf, &[], now()).is_ok());
}

fn lived(seconds: u64) -> CertificateParams {
    let mut params = conforming_params(&[AGENT_URI]);
    let now = std::time::SystemTime::now();
    params.not_before = (now - std::time::Duration::from_secs(60)).into();
    params.not_after = (now - std::time::Duration::from_secs(60) + std::time::Duration::from_secs(seconds)).into();
    params
}

#[test]
fn accepts_a_client_certificate_valid_for_exactly_the_ceiling() {
    let anchor = ca("anchor");
    let leaf = leaf_from(&anchor, lived(3600));
    assert!(verifier(&anchor).verify_client_cert(&leaf, &[], now()).is_ok());
}

#[test]
fn refuses_a_client_certificate_valid_for_longer_than_the_ceiling_with_a_typed_error() {
    let anchor = ca("anchor");
    let verifier = verifier(&anchor);
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    let mut decades = conforming_params(&[AGENT_URI]);
    decades.not_before = (std::time::SystemTime::now() - day).into();
    decades.not_after = (std::time::SystemTime::now() + 365 * day).into();
    for params in [lived(3601), decades] {
        let leaf = leaf_from(&anchor, params);
        let error = verifier.verify_client_cert(&leaf, &[], now()).expect_err("refused");
        assert!(
            matches!(
                &error,
                rustls::Error::InvalidCertificate(CertificateError::Other(other))
                    if matches!(
                        other.0.downcast_ref::<SvidLifetimeError>(),
                        Some(SvidLifetimeError::TooLong { .. })
                    )
            ),
            "{error:?}"
        );
    }
}

#[test]
fn a_listener_refuses_an_over_long_client_certificate_in_the_handshake() {
    let anchor = ca("anchor");
    assert!(!refused_in_the_handshake(&anchor, lived(3600)));
    assert!(refused_in_the_handshake(&anchor, lived(3601)));
}

#[test]
fn startup_refuses_a_server_certificate_outside_the_svid_profile_or_lifetime() {
    let anchor = ca("anchor");
    let mut too_long = server_params();
    too_long.not_after = (std::time::SystemTime::now() + std::time::Duration::from_secs(2 * 3600)).into();
    let mut no_signature = server_params();
    no_signature.key_usages = vec![KeyUsagePurpose::KeyEncipherment];
    for (params, expected_lifetime) in [(too_long, true), (no_signature, false)] {
        let key = KeyPair::generate().expect("key");
        let certificate = params.signed_by(&key, &anchor).expect("server certificate");
        let error = server_config(
            vec![certificate.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            vec![anchor.der().clone()],
            Path::new("test-ca.pem"),
            domain(),
        )
        .expect_err("refused");
        match (expected_lifetime, &error) {
            (true, TlsError::ServerCertificateLifetime(SvidLifetimeError::TooLong { .. }))
            | (false, TlsError::ServerCertificateProfile(MaterialError::Profile)) => {}
            _ => panic!("{error:?}"),
        }
    }
}

fn deliver(from: &mut Connection, to: &mut Connection) -> Result<(), rustls::Error> {
    let mut flight = Vec::new();
    from.write_tls(&mut flight).expect("TLS flight");
    let mut flight = flight.as_slice();
    while !flight.is_empty() {
        to.read_tls(&mut flight).expect("receive TLS flight");
        to.process_new_packets()?;
    }
    Ok(())
}

fn refused_in_the_handshake(anchor: &CertifiedIssuer<'static, KeyPair>, params: CertificateParams) -> bool {
    let server_params = server_params();
    let server_key = KeyPair::generate().expect("server key");
    let server_certificate = server_params
        .signed_by(&server_key, anchor)
        .expect("server certificate");
    let server = Arc::new(
        server_config(
            vec![server_certificate.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
            vec![anchor.der().clone()],
            Path::new("test-ca.pem"),
            domain(),
        )
        .expect("listener TLS configuration"),
    );
    let client_key = KeyPair::generate().expect("client key");
    let client_certificate = params.signed_by(&client_key, anchor).expect("client certificate");
    let mut roots = RootCertStore::empty();
    roots.add(anchor.der().clone()).expect("server trust");
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let client = Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocols")
            .with_root_certificates(roots)
            .with_client_auth_cert(
                vec![client_certificate.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(client_key.serialize_der())),
            )
            .expect("client authentication"),
    );
    let mut client = Connection::Client(
        ClientConnection::new(client, "proxy.example".try_into().expect("server name")).expect("TLS client"),
    );
    let mut server = Connection::Server(ServerConnection::new(server).expect("TLS server"));
    for _ in 0..16 {
        if deliver(&mut client, &mut server).is_err() || deliver(&mut server, &mut client).is_err() {
            return true;
        }
        if !client.is_handshaking() && !server.is_handshaking() {
            return false;
        }
    }
    panic!("the handshake neither completed nor failed");
}

#[test]
fn the_handshake_refuses_a_leaf_that_breaks_the_svid_key_constraints() {
    let anchor = ca("anchor");
    for (name, params) in violations() {
        assert!(refused_in_the_handshake(&anchor, params), "{name} must be refused");
    }
    assert!(!refused_in_the_handshake(&anchor, conforming_params(&[AGENT_URI])));
}

#[test]
fn client_authentication_is_mandatory() {
    let anchor = ca("anchor");
    let verifier = verifier(&anchor);
    assert!(verifier.offer_client_auth());
    assert!(verifier.client_auth_mandatory());
}

#[test]
fn peer_agent_reads_the_first_certificate() {
    let anchor = ca("anchor");
    let leaf = client_leaf(&anchor, &["spiffe://trogon.example/agent/agent-1"]);
    let chain = [leaf, anchor.der().clone()];
    assert_eq!(
        peer_agent(Some(&chain), &domain()),
        Ok(AgentId::new("agent-1").expect("agent"))
    );
    assert_eq!(peer_agent(None, &domain()), Err(PeerIdentityError::NoUriSan));
    assert_eq!(peer_agent(Some(&[]), &domain()), Err(PeerIdentityError::NoUriSan));
}

#[test]
fn loading_reports_missing_empty_and_keyless_files() {
    let dir = tempfile::tempdir().expect("dir");
    let missing = dir.path().join("missing.pem");
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "").expect("write");

    assert!(matches!(read_certificates(&missing), Err(TlsError::Unreadable { .. })));
    assert!(matches!(read_certificates(&empty), Err(TlsError::NoCertificate { .. })));
    assert!(matches!(read_private_key(&empty), Err(TlsError::NoPrivateKey { .. })));
    assert!(matches!(read_private_key(&missing), Err(TlsError::Unreadable { .. })));
}

#[test]
fn a_path_that_opens_but_cannot_be_read_is_unreadable() {
    let dir = tempfile::tempdir().expect("dir");

    assert!(matches!(
        read_certificates(dir.path()),
        Err(TlsError::Unreadable { .. })
    ));
    assert!(matches!(read_private_key(dir.path()), Err(TlsError::Unreadable { .. })));
}

#[test]
fn a_trust_anchor_that_is_not_a_certificate_is_refused_and_named() {
    let anchors = vec![CertificateDer::from(vec![0u8; 8])];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(Vec::new()));

    let error = server_config(Vec::new(), key, anchors, Path::new("anchors.pem"), domain()).expect_err("refused");

    assert!(
        matches!(&error, TlsError::InvalidTrustAnchor { path, .. } if path == Path::new("anchors.pem")),
        "{error:?}"
    );
}

fn ca_naming(uris: &[&str]) -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "named-ca");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.subject_alt_names = uris
        .iter()
        .map(|uri| SanType::URI(Ia5String::try_from(*uri).expect("ia5")))
        .collect();
    CertifiedIssuer::self_signed(params, KeyPair::generate().expect("key")).expect("ca")
}

fn config_with_anchors(anchors: &[&CertifiedIssuer<'static, KeyPair>]) -> Result<ServerConfig, TlsError> {
    let signer = ca("signer");
    let key = KeyPair::generate().expect("key");
    let server = server_params().signed_by(&key, &signer).expect("server");
    server_config(
        vec![server.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        anchors.iter().map(|anchor| anchor.der().clone()).collect(),
        Path::new("anchors.pem"),
        domain(),
    )
}

#[test]
fn an_anchor_whose_san_is_not_a_uri_is_accepted() {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "dns-ca");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.subject_alt_names = vec![SanType::DnsName(Ia5String::try_from("ca.example").expect("ia5"))];
    let anchor = CertifiedIssuer::self_signed(params, KeyPair::generate().expect("key")).expect("ca");
    assert!(config_with_anchors(&[&anchor]).is_ok());
}

/// Parses as X.509, but the outer signature algorithm differs from the one the
/// certificate signs with, which the trust store refuses.
fn anchor_with_mismatched_signature_algorithms() -> CertificateDer<'static> {
    // ecdsa-with-SHA256 and ecdsa-with-SHA384 object identifiers.
    const SHA256: [u8; 10] = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
    let mut der = ca_naming(&["spiffe://trogon.example"]).der().to_vec();
    let outer = der
        .windows(SHA256.len())
        .rposition(|window| window == SHA256)
        .expect("outer signature algorithm");
    der[outer + SHA256.len() - 1] = 0x03;
    CertificateDer::from(der)
}

#[test]
fn an_anchor_the_trust_store_refuses_is_named_in_the_error() {
    let signer = ca("signer");
    let key = KeyPair::generate().expect("key");
    let server = server_params().signed_by(&key, &signer).expect("server");
    let error = server_config(
        vec![server.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        vec![anchor_with_mismatched_signature_algorithms()],
        Path::new("anchors.pem"),
        domain(),
    )
    .expect_err("refused");
    assert!(
        matches!(&error, TlsError::InvalidTrustAnchor { path, .. } if path == Path::new("anchors.pem")),
        "{error:?}"
    );
}

#[test]
fn an_anchor_naming_exactly_the_configured_trust_domain_is_accepted() {
    let own = ca_naming(&["spiffe://trogon.example"]);
    assert!(config_with_anchors(&[&own]).is_ok());
}

#[test]
fn an_anchor_without_a_uri_san_is_accepted() {
    let upstream = ca("upstream-root");
    assert!(config_with_anchors(&[&upstream]).is_ok());
}

#[test]
fn an_anchor_naming_another_trust_domain_is_refused_even_beside_a_valid_one() {
    let own = ca_naming(&["spiffe://trogon.example"]);
    let foreign = ca_naming(&["spiffe://other.example"]);
    let error = config_with_anchors(&[&own, &foreign]).expect_err("refused");
    assert!(
        matches!(
            &error,
            TlsError::ForeignTrustAnchor { path, uri, trust_domain }
                if path == Path::new("anchors.pem")
                    && uri == "spiffe://other.example"
                    && trust_domain.as_str() == "trogon.example"
        ),
        "{error:?}"
    );
}

#[test]
fn an_anchor_naming_a_path_or_a_lookalike_domain_is_refused() {
    for uri in [
        "spiffe://trogon.example/agent/agent-1",
        "spiffe://trogon.example/",
        "spiffe://trogon.example.evil",
        "spiffe://trogon.exampl",
        "https://trogon.example",
    ] {
        let anchor = ca_naming(&[uri]);
        assert!(
            matches!(
                config_with_anchors(&[&anchor]),
                Err(TlsError::ForeignTrustAnchor { .. })
            ),
            "{uri}"
        );
    }
}

#[test]
fn an_anchor_with_more_than_one_uri_san_is_refused() {
    let anchor = ca_naming(&["spiffe://trogon.example", "spiffe://other.example"]);
    let error = config_with_anchors(&[&anchor]).expect_err("refused");
    assert!(
        matches!(&error, TlsError::AmbiguousTrustAnchor { path } if path == Path::new("anchors.pem")),
        "{error:?}"
    );
}

/// A signature structure as it appears on the wire: scheme, length, bytes.
fn forged_signature() -> DigitallySignedStruct {
    let wire = [0x04, 0x03, 0x00, 0x04, 1, 2, 3, 4];
    DigitallySignedStruct::read(&mut Reader::init(&wire)).expect("signature structure")
}

#[test]
fn a_forged_handshake_signature_is_refused_in_both_protocol_versions() {
    let anchor = ca("anchor");
    let leaf = client_leaf(&anchor, &["spiffe://trogon.example/agent/agent-1"]);
    let verifier = verifier(&anchor);

    assert!(
        verifier
            .verify_tls12_signature(b"transcript", &leaf, &forged_signature())
            .is_err()
    );
    assert!(
        verifier
            .verify_tls13_signature(b"transcript", &leaf, &forged_signature())
            .is_err()
    );
}

#[test]
fn a_full_configuration_loads_from_pem_files() {
    let dir = tempfile::tempdir().expect("dir");
    let anchor = ca("anchor");
    let params = server_params();
    let key = KeyPair::generate().expect("key");
    let server = params.signed_by(&key, &anchor).expect("server");

    let files = TlsFiles {
        certificate_chain: dir.path().join("server.pem"),
        private_key: dir.path().join("server.key"),
        client_trust_anchors: dir.path().join("ca.pem"),
    };
    std::fs::write(&files.certificate_chain, server.pem()).expect("write");
    std::fs::write(&files.private_key, key.serialize_pem()).expect("write");
    std::fs::write(&files.client_trust_anchors, anchor.pem()).expect("write");

    assert!(load_server_config(&files, domain()).is_ok());

    std::fs::write(&files.client_trust_anchors, "").expect("write");
    assert!(matches!(
        load_server_config(&files, domain()),
        Err(TlsError::NoCertificate { .. })
    ));
}

fn exchange(from: &mut Connection, to: &mut Connection) -> usize {
    let mut flight = Vec::new();
    let written = from.write_tls(&mut flight).expect("TLS flight");
    if !flight.is_empty() {
        let mut flight = flight.as_slice();
        while !flight.is_empty() {
            assert!(to.read_tls(&mut flight).expect("receive TLS flight") > 0);
            to.process_new_packets().expect("authenticate TLS flight");
        }
    }
    written
}

fn handshake(client: Arc<ClientConfig>, server: Arc<ServerConfig>) -> (ProtocolVersion, HandshakeKind) {
    let mut client = Connection::Client(
        ClientConnection::new(client, "proxy.example".try_into().expect("server name")).expect("TLS client"),
    );
    let mut server = Connection::Server(ServerConnection::new(server).expect("TLS server"));
    while client.is_handshaking() || server.is_handshaking() || client.wants_write() || server.wants_write() {
        let progress = exchange(&mut client, &mut server) + exchange(&mut server, &mut client);
        assert!(progress > 0, "the handshake must make progress");
    }
    assert_eq!(client.handshake_kind(), server.handshake_kind());
    assert_eq!(
        peer_agent(server.peer_certificates(), &domain()),
        Ok(AgentId::new("agent-1").expect("agent"))
    );
    (
        server.protocol_version().expect("negotiated protocol"),
        server.handshake_kind().expect("completed handshake"),
    )
}

fn subsequent_handshakes(configure: impl Fn(&mut ServerConfig)) -> Vec<(ProtocolVersion, HandshakeKind)> {
    rustls::ALL_VERSIONS
        .iter()
        .map(|version| {
            let anchor = ca("resumption-test");
            let params = server_params();
            let server_key = KeyPair::generate().expect("server key");
            let certificate = params.signed_by(&server_key, &anchor).expect("server certificate");
            let mut server = server_config(
                vec![certificate.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
                vec![anchor.der().clone()],
                Path::new("test-ca.pem"),
                domain(),
            )
            .expect("listener TLS configuration");
            configure(&mut server);
            let server = Arc::new(server);

            let params = conforming_params(&[AGENT_URI]);
            let client_key = KeyPair::generate().expect("client key");
            let certificate = params.signed_by(&client_key, &anchor).expect("client certificate");
            let mut roots = RootCertStore::empty();
            roots.add(anchor.der().clone()).expect("server trust");
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let client = Arc::new(
                ClientConfig::builder_with_provider(provider)
                    .with_protocol_versions(&[version])
                    .expect("supported protocol")
                    .with_root_certificates(roots)
                    .with_client_auth_cert(
                        vec![certificate.der().clone()],
                        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(client_key.serialize_der())),
                    )
                    .expect("client authentication"),
            );

            assert_eq!(
                handshake(Arc::clone(&client), Arc::clone(&server)),
                (version.version, HandshakeKind::Full)
            );
            handshake(client, server)
        })
        .collect()
}

#[test]
fn every_connection_requires_a_fresh_mutual_tls_handshake() {
    let handshakes = subsequent_handshakes(|_| {});
    let expected: Vec<_> = rustls::ALL_VERSIONS
        .iter()
        .map(|version| (version.version, HandshakeKind::Full))
        .collect();
    assert_eq!(handshakes, expected);
}

#[test]
fn handshake_fixture_can_resume_when_the_listener_explicitly_allows_it() {
    let handshakes = subsequent_handshakes(|config| {
        config.session_storage = rustls::server::ServerSessionMemoryCache::new(256);
        config.send_tls13_tickets = 2;
    });
    let expected: Vec<_> = rustls::ALL_VERSIONS
        .iter()
        .map(|version| (version.version, HandshakeKind::Resumed))
        .collect();
    assert_eq!(handshakes, expected);
}
