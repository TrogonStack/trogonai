use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType, string::Ia5String,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;

use super::*;
use crate::audit::{AuditEvent, AuditSink};
use crate::audit_error::AuditError;
use crate::limits::Limits;
use crate::mtls::server_config;
use crate::policy_source::PolicySource;
use crate::resolver::SystemResolver;
use crate::static_snapshot::StaticSnapshot;

const WAIT: Duration = Duration::from_secs(5);
const TRUST_DOMAIN: &str = "trogon.example";

#[derive(Default)]
struct Decisions(AtomicUsize);

#[async_trait]
impl AuditSink for Decisions {
    async fn record(&self, _: &AuditEvent<'_>) -> Result<(), AuditError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A sink with background work: it notes that it was told to flush, and only
/// then that it finished.
#[derive(Default)]
struct Draining {
    flushed: AtomicBool,
}

#[async_trait]
impl AuditSink for Draining {
    async fn record(&self, _: &AuditEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }

    async fn run(&self, shutdown: CancellationToken) {
        shutdown.cancelled().await;
        self.flushed.store(true, Ordering::SeqCst);
    }
}

/// A sink whose background work dies.
struct Crashing;

#[async_trait]
impl AuditSink for Crashing {
    async fn record(&self, _: &AuditEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }

    async fn run(&self, _: CancellationToken) {
        panic!("the drain task crashed");
    }
}

impl Decisions {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

struct Identity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

struct Pki {
    ca: CertifiedIssuer<'static, KeyPair>,
}

impl Pki {
    fn new() -> Self {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "test-ca");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        Self {
            ca: CertifiedIssuer::self_signed(params, KeyPair::generate().expect("key")).expect("ca"),
        }
    }

    fn issue(&self, params: CertificateParams) -> Identity {
        let key = KeyPair::generate().expect("key");
        let leaf = params.signed_by(&key, &self.ca).expect("leaf");
        Identity {
            chain: vec![leaf.der().clone()],
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        }
    }

    fn server(&self) -> Identity {
        let mut params = CertificateParams::new(vec!["localhost".to_owned()]).expect("params");
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        self.issue(params)
    }

    fn client(&self, uris: &[&str]) -> Identity {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names = uris
            .iter()
            .map(|uri| SanType::URI(Ia5String::try_from(*uri).expect("ia5")))
            .collect();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        self.issue(params)
    }

    fn roots(&self) -> RootCertStore {
        let mut roots = RootCertStore::empty();
        roots.add(self.ca.der().clone()).expect("root");
        roots
    }

    /// The configuration the proxy runs with: a certificate that names no
    /// single agent is already refused in the handshake.
    fn strict_config(&self) -> ServerConfig {
        let server = self.server();
        let trust_domain = TrustDomain::new(TRUST_DOMAIN).expect("trust domain");
        server_config(
            server.chain,
            server.key,
            vec![self.ca.der().clone()],
            Path::new("ca.pem"),
            trust_domain,
        )
        .expect("config")
    }

    /// A configuration whose handshake accepts any certificate the CA issued,
    /// so what the accept loop does with the identity is the only gate left.
    fn lax_config(&self) -> ServerConfig {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(self.roots()), Arc::clone(&provider))
            .build()
            .expect("verifier");
        let server = self.server();
        ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_client_cert_verifier(verifier)
            .with_single_cert(server.chain, server.key)
            .expect("config")
    }

    fn connector(&self, identity: Identity) -> TlsConnector {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(self.roots())
            .with_client_auth_cert(identity.chain, identity.key)
            .expect("client auth");
        TlsConnector::from(Arc::new(config))
    }
}

async fn server(tls: ServerConfig, limits: Limits) -> (Server, Arc<Decisions>) {
    let decisions = Arc::new(Decisions::default());
    (audited_server(tls, limits, decisions.clone()).await, decisions)
}

async fn audited_server(tls: ServerConfig, limits: Limits, audit: Arc<dyn AuditSink>) -> Server {
    let snapshot = StaticSnapshot::from_toml(
        PolicySource::StaticFile {
            path: PathBuf::from("policy.toml"),
        },
        "[agents.agent-1]\nallow = [\"example.com:443\"]\n",
    )
    .expect("policy");
    let proxy = Proxy::new(
        Arc::new(snapshot),
        Arc::new(SystemResolver),
        audit,
        limits,
        Default::default(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let trust_domain = TrustDomain::new(TRUST_DOMAIN).expect("trust domain");
    Server::new(listener, Arc::new(tls), proxy, trust_domain)
}

/// Reads until the proxy closes the connection, however it does.
async fn drain<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
    let mut received = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut received))
        .await
        .expect("the proxy closes the connection")
        .ok();
    received
}

async fn stop(shutdown: CancellationToken, task: JoinHandle<()>) {
    shutdown.cancel();
    task.await.expect("accept loop ends");
}

#[tokio::test]
async fn a_client_that_never_finishes_the_handshake_is_dropped() {
    let limits = Limits {
        handshake_timeout: Duration::from_millis(50),
        ..Limits::default()
    };
    let (server, decisions) = server(Pki::new().strict_config(), limits).await;
    let address = server.local_addr().expect("addr");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));

    let mut client = TcpStream::connect(address).await.expect("connect");
    assert!(drain(&mut client).await.is_empty());
    assert_eq!(decisions.count(), 0);
    stop(shutdown, task).await;
}

#[tokio::test]
async fn the_audit_drain_runs_while_serving_and_has_flushed_when_run_returns() {
    let audit = Arc::new(Draining::default());
    let server = audited_server(Pki::new().strict_config(), Limits::default(), audit.clone()).await;
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));

    tokio::task::yield_now().await;
    assert!(!audit.flushed.load(Ordering::SeqCst), "not before shutdown");
    stop(shutdown, task).await;
    assert!(audit.flushed.load(Ordering::SeqCst), "run returns only after the flush");
}

#[tokio::test]
async fn a_crashed_audit_drain_does_not_take_the_server_down_with_it() {
    let server = audited_server(Pki::new().strict_config(), Limits::default(), Arc::new(Crashing)).await;
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    stop(shutdown, task).await;
}

struct Flaky {
    failures: Arc<AtomicUsize>,
    listener: TcpListener,
}

impl Accept for Flaky {
    async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let failing = self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1));
        if failing.is_ok() {
            return Err(io::Error::other("too many open files"));
        }
        self.listener.accept().await
    }
}

#[tokio::test]
async fn the_accept_loop_outlives_failed_accepts() {
    let (server, decisions) = server(Pki::new().strict_config(), Limits::default()).await;
    let address = server.local_addr().expect("addr");
    let Server {
        listener,
        acceptor,
        proxy,
        trust_domain,
    } = server;
    let failures = Arc::new(AtomicUsize::new(2));
    let flaky = Flaky {
        failures: Arc::clone(&failures),
        listener,
    };
    let shutdown = CancellationToken::new();
    let task = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { accept_loop(&flaky, &acceptor, &proxy, &trust_domain, shutdown).await }
    });

    let mut client = TcpStream::connect(address).await.expect("connect");
    client.write_all(b"not tls\r\n\r\n").await.expect("write");
    drain(&mut client).await;

    assert_eq!(failures.load(Ordering::SeqCst), 0, "both failures were met first");
    assert_eq!(decisions.count(), 0, "a handshake that fails never reaches admission");
    stop(shutdown, task).await;
}

#[tokio::test]
async fn a_certificate_naming_no_agent_gets_no_answer() {
    let pki = Pki::new();
    let (server, decisions) = server(pki.lax_config(), Limits::default()).await;
    let address = server.local_addr().expect("addr");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let name = ServerName::try_from("localhost").expect("name");
    let request = b"CONNECT other.example:443 HTTP/1.1\r\n\r\n";

    let agent = pki.client(&["spiffe://trogon.example/agent/agent-1"]);
    let tcp = TcpStream::connect(address).await.expect("connect");
    let mut stream = pki
        .connector(agent)
        .connect(name.clone(), tcp)
        .await
        .expect("handshake");
    stream.write_all(request).await.expect("write");
    let answered = String::from_utf8(drain(&mut stream).await).expect("utf8");
    assert!(answered.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{answered}");

    let nobody = pki.client(&[]);
    let tcp = TcpStream::connect(address).await.expect("connect");
    let mut stream = pki.connector(nobody).connect(name, tcp).await.expect("handshake");
    stream.write_all(request).await.ok();
    assert!(drain(&mut stream).await.is_empty());

    assert_eq!(decisions.count(), 1, "only the named agent was admitted to a decision");
    stop(shutdown, task).await;
}
