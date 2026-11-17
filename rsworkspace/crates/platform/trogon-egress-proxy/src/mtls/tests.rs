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

fn client_leaf(issuer: &CertifiedIssuer<'static, KeyPair>, uris: &[&str]) -> CertificateDer<'static> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.subject_alt_names = uris
        .iter()
        .map(|uri| SanType::URI(Ia5String::try_from(*uri).expect("ia5")))
        .collect();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let key = KeyPair::generate().expect("key");
    params.signed_by(&key, issuer).expect("leaf").der().clone()
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
    let mut params = CertificateParams::new(vec!["proxy.example".to_owned()]).expect("params");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
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
            let mut params = CertificateParams::new(vec!["proxy.example".to_owned()]).expect("server params");
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
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

            let mut params = CertificateParams::default();
            params.distinguished_name = DistinguishedName::new();
            params.subject_alt_names = vec![SanType::URI(
                Ia5String::try_from("spiffe://trogon.example/agent/agent-1").expect("agent URI"),
            )];
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
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
