use std::num::NonZeroU64;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::UnixTime;
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::io::AsyncWriteExt;
use tracing::Level;
use trogon_workload_identity::test_support::{TestCa, WorkloadApiFixture};
use trogon_workload_identity::{IdentityConfig, IdentityStore, RegistrationBinding, RegistrationSet, WorkloadRole};

use super::refusal_log_tests::client_params;
use super::*;
use crate::enforcement_mode::EnforcementMode;
use crate::telemetry::metrics::ConnectionMetrics;
use crate::telemetry::recorded_metrics::RecordedMetrics;
use trogon_semconv::metric::EGRESS_PROXY_CONNECTIONS_ACCEPTED;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

const AGENT_ID: &str = "spiffe://trogon.example/agent/agent-1";
const PEER_ID: &str = "spiffe://trogon.example/runtime/peer";
const LOCAL_ID: &str = "spiffe://trogon.example/service/egress";

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

struct ActivationAcknowledged;

#[async_trait]
impl trogon_workload_identity::ActivationAudit for ActivationAcknowledged {
    async fn record(
        &self,
        _: &trogon_workload_identity::ActivationFact,
    ) -> Result<(), trogon_workload_identity::ActivationAuditError> {
        Ok(())
    }
}

fn metered_proxy(metrics: ConnectionMetrics) -> Arc<Proxy> {
    let snapshot = StaticSnapshot::from_toml(
        PolicySource::StaticFile {
            path: PathBuf::from("policy.toml"),
        },
        "[agents.agent-1]\nallow = [\"example.com:443\"]\n",
    )
    .expect("policy");
    Arc::new(
        Proxy::new(
            Arc::new(snapshot),
            Arc::new(SystemResolver),
            Arc::new(Decisions::default()),
            Limits::default(),
            Default::default(),
        )
        .with_metrics(metrics),
    )
}

/// Connects one client through `connector` and serves the accepted socket.
async fn serve_one(source: ListenerIdentity, proxy: Arc<Proxy>, connector: TlsConnector) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let client = tokio::spawn(async move {
        let tcp = TcpStream::connect(address).await.expect("connect");
        let mut stream = connector
            .connect(ServerName::try_from("localhost").expect("name"), tcp)
            .await
            .expect("handshake");
        stream
            .write_all(b"CONNECT other.example:443 HTTP/1.1\r\n\r\n")
            .await
            .ok();
        drain(&mut stream).await
    });
    let (stream, peer) = listener.accept().await.expect("accept");
    let trust_domain = Arc::new(TrustDomain::new(TRUST_DOMAIN).expect("trust domain"));
    serve_connection(source, proxy, trust_domain, stream, peer).await;
    client.await.expect("client");
}

fn static_source(pki: &Pki) -> ListenerIdentity {
    ListenerIdentity::Fixed(TlsAcceptor::from(Arc::new(pki.strict_config())))
}

async fn workload_source(ca: &TestCa, fixture: &WorkloadApiFixture) -> (ListenerIdentity, CancellationToken) {
    let local = RegistrationBinding::new(LOCAL_ID, "acme", "payments", WorkloadRole::EgressProxy, None).expect("local");
    let peer =
        RegistrationBinding::new(PEER_ID, "acme", "payments", WorkloadRole::Agent, Some("agent-1")).expect("peer");
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
                std::time::SystemTime::now() + Duration::from_secs(300),
                vec![local, peer],
                vec![ca.root_sha256()],
            )
            .expect("registration"),
        )
        .expect("governed authority");
    let shutdown = CancellationToken::new();
    tokio::spawn(driver.run(shutdown.clone()));
    tokio::time::timeout(WAIT, async {
        while identity.material().is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("approved material");
    (ListenerIdentity::Workload(identity), shutdown)
}

fn trusting_any_server(identity: &trogon_workload_identity::test_support::TestIdentity) -> TlsConnector {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = AcceptAnyServer(provider.signature_verification_algorithms);
    TlsConnector::from(Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_client_auth_cert(identity.certificates(), identity.private_key())
            .expect("client auth"),
    ))
}

fn accepted_events(captured: &CapturedEvents) -> Vec<trogon_std::log_capture::CapturedEvent> {
    captured
        .at(Level::INFO)
        .into_iter()
        .filter(|event| event.field("event") == Some("peer_connection_accepted"))
        .collect()
}

#[tokio::test]
async fn a_static_file_connection_is_logged_and_counted_with_its_full_peer_id() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let recorded = RecordedMetrics::new();
    let pki = Pki::new();
    let connector = pki.connector(pki.issue(client_params(AGENT_ID)));

    serve_one(
        static_source(&pki),
        metered_proxy(ConnectionMetrics::new(&recorded.meter())),
        connector,
    )
    .await;

    let events = accepted_events(&captured);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].field("peer.spiffe_id"), Some(AGENT_ID));
    assert_eq!(events[0].field("credential_source"), Some("static_file"));
    assert!(
        events[0]
            .field("peer")
            .is_some_and(|peer| peer.starts_with("127.0.0.1:"))
    );
    assert_eq!(
        recorded.counts(EGRESS_PROXY_CONNECTIONS_ACCEPTED),
        [(vec!["egress_proxy.credential_source=static_file".to_owned()], 1)]
    );
}

#[tokio::test]
async fn a_workload_api_connection_is_logged_and_counted_with_its_full_peer_id() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let recorded = RecordedMetrics::new();
    let ca = TestCa::new();
    let local = ca.issue(LOCAL_ID, Duration::from_secs(600));
    let peer = ca.issue(PEER_ID, Duration::from_secs(600));
    let fixture = WorkloadApiFixture::start(vec![local]).await;
    let (source, shutdown) = workload_source(&ca, &fixture).await;

    serve_one(
        source,
        metered_proxy(ConnectionMetrics::new(&recorded.meter())),
        trusting_any_server(&peer),
    )
    .await;
    shutdown.cancel();
    fixture.stop().await;

    let events = accepted_events(&captured);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].field("peer.spiffe_id"), Some(PEER_ID));
    assert_eq!(events[0].field("credential_source"), Some("workload_api"));
    assert_eq!(
        recorded.counts(EGRESS_PROXY_CONNECTIONS_ACCEPTED),
        [(vec!["egress_proxy.credential_source=workload_api".to_owned()], 1)]
    );
}

async fn refused_peer(mode: EnforcementMode, identity: Option<&str>) {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let recorded = RecordedMetrics::new();
    let pki = Pki::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let connector = match identity {
        Some(uri) => pki.connector(pki.issue(client_params(uri))),
        None => anonymous(&pki),
    };
    let client = tokio::spawn(async move {
        let tcp = TcpStream::connect(address).await.expect("connect");
        if let Ok(mut stream) = connector
            .connect(ServerName::try_from("localhost").expect("name"), tcp)
            .await
        {
            stream.write_all(b"CONNECT a:443 HTTP/1.1\r\n\r\n").await.ok();
            assert!(drain(&mut stream).await.is_empty());
        }
    });
    let (stream, peer) = listener.accept().await.expect("accept");
    let proxy = Arc::try_unwrap(metered_proxy(ConnectionMetrics::new(&recorded.meter())))
        .unwrap_or_else(|_| unreachable!("sole owner"))
        .with_mode(mode);
    serve_connection(
        static_source(&pki),
        Arc::new(proxy),
        Arc::new(TrustDomain::new(TRUST_DOMAIN).expect("trust domain")),
        stream,
        peer,
    )
    .await;
    client.await.expect("client");

    assert!(accepted_events(&captured).is_empty());
    assert!(recorded.counts(EGRESS_PROXY_CONNECTIONS_ACCEPTED).is_empty());
}

fn anonymous(pki: &Pki) -> TlsConnector {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    TlsConnector::from(Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(pki.roots())
            .with_no_client_auth(),
    ))
}

#[tokio::test]
async fn a_peer_from_another_trust_domain_is_refused_in_every_mode_and_never_counted() {
    for mode in [EnforcementMode::Enforce, EnforcementMode::DryRun] {
        refused_peer(mode, Some("spiffe://other.example/agent/agent-1")).await;
    }
}

#[tokio::test]
async fn an_unauthenticated_peer_is_refused_in_every_mode_and_never_counted() {
    for mode in [EnforcementMode::Enforce, EnforcementMode::DryRun] {
        refused_peer(mode, None).await;
    }
}
