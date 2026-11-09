//! Shared fixtures: an in-test CA, a running proxy on 127.0.0.1, clients, and
//! a recording upstream. All key material is generated per run.

#![allow(dead_code, clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType, string::Ia5String,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_util::sync::CancellationToken;
use trogon_audit::{AuditName, WriterId, WriterIdentity};
use trogon_egress_proxy::audit::{AuditEvent, AuditSink, Decision};
use trogon_egress_proxy::audit_config::AuditConfig;
use trogon_egress_proxy::audit_error::AuditError;
use trogon_egress_proxy::config::{Config, PolicyInput};
use trogon_egress_proxy::host_name::HostName;
use trogon_egress_proxy::limits::Limits;
use trogon_egress_proxy::mtls::TlsFiles;
use trogon_egress_proxy::resolver::Resolver;
use trogon_egress_proxy::runtime::start_with;
use trogon_egress_proxy::trust_domain::TrustDomain;
use trogon_nats::{NatsConfig, NatsToken};

pub const TRUST_DOMAIN: &str = "trogon.example";
pub const WAIT: Duration = Duration::from_secs(5);

pub struct Pki {
    ca: CertifiedIssuer<'static, KeyPair>,
}

pub struct ClientIdentity {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

impl Pki {
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().expect("key")).expect("ca");
        Self { ca }
    }

    pub fn ca_pem(&self) -> String {
        self.ca.pem()
    }

    /// A client certificate carrying exactly the given URI SANs.
    pub fn client(&self, uris: &[&str]) -> ClientIdentity {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names = uris
            .iter()
            .map(|uri| SanType::URI(Ia5String::try_from(*uri).expect("ia5")))
            .collect();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let key = KeyPair::generate().expect("key");
        let leaf = params.signed_by(&key, &self.ca).expect("leaf");
        ClientIdentity {
            chain: vec![leaf.der().clone()],
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        }
    }

    pub fn agent(&self, agent: &str) -> ClientIdentity {
        self.client(&[&format!("spiffe://{TRUST_DOMAIN}/agent/{agent}")])
    }

    /// A complete configuration over fresh files in `dir`. Listens on `listen`
    /// and enforces `policy`; every other setting is the default.
    pub fn config(&self, dir: &std::path::Path, policy: &str, listen: std::net::SocketAddr) -> Config {
        let policy_file: PathBuf = dir.join("policy.toml");
        std::fs::write(&policy_file, policy).expect("write");
        Config {
            listen,
            policy: PolicyInput::StaticFile(policy_file),
            tls: trogon_egress_proxy::config::TlsInput::Files(self.write_server_files(dir)),
            trust_domain: TrustDomain::new(TRUST_DOMAIN).expect("trust domain"),
            audit: audit_config(),
            non_tls_ports: Vec::new(),
            limits: Limits::default(),
        }
    }

    fn write_server_files(&self, dir: &std::path::Path) -> TlsFiles {
        let mut params = CertificateParams::new(vec!["localhost".to_owned()]).expect("params");
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().expect("key");
        let server = params.signed_by(&key, &self.ca).expect("server");
        let files = TlsFiles {
            certificate_chain: dir.join("server.pem"),
            private_key: dir.join("server.key"),
            client_trust_anchors: dir.join("client-ca.pem"),
        };
        std::fs::write(&files.certificate_chain, server.pem()).expect("write");
        std::fs::write(&files.private_key, key.serialize_pem()).expect("write");
        std::fs::write(&files.client_trust_anchors, self.ca.pem()).expect("write");
        files
    }
}

/// Audit settings that name a stream nobody listens on: tests that start the
/// proxy through `start_with` bring their own sink, and `start` does not need
/// the stream to answer.
pub fn audit_config() -> AuditConfig {
    AuditConfig {
        nats: NatsConfig::from_url("nats://127.0.0.1:1"),
        identity: WriterIdentity::new(
            WriterId::new("egress-proxy:test").expect("writer"),
            NatsToken::new("egress_proxy").expect("component"),
            AuditName::new("acme").expect("tenant"),
            NatsToken::new("payments").expect("project"),
        ),
    }
}

type Record = (Option<String>, Option<IpAddr>, Decision);

#[derive(Default)]
pub struct Recorder(Mutex<Vec<Record>>);

#[async_trait]
impl AuditSink for Recorder {
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError> {
        self.0.lock().expect("lock").push((
            event.destination.map(ToString::to_string),
            event.resolved_address,
            event.decision,
        ));
        Ok(())
    }
}

impl Recorder {
    pub fn events(&self) -> Vec<(Option<String>, Option<IpAddr>, Decision)> {
        self.0.lock().expect("lock").clone()
    }
}

pub struct Names(pub HashMap<String, IpAddr>);

#[async_trait]
impl Resolver for Names {
    async fn resolve(&self, host: &HostName) -> io::Result<Vec<IpAddr>> {
        self.0
            .get(host.as_str())
            .map(|ip| vec![*ip])
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown name"))
    }
}

pub struct Harness {
    pub address: std::net::SocketAddr,
    pub audit: Arc<Recorder>,
    pub pki: Pki,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
    _dir: tempfile::TempDir,
}

pub struct Setup<'a> {
    pub policy: &'a str,
    pub names: &'a [(&'a str, &'a str)],
    pub non_tls_ports: &'a [u16],
}

impl Harness {
    pub async fn start(setup: Setup<'_>) -> Self {
        let dir = tempfile::tempdir().expect("dir");
        let pki = Pki::new("test-ca");
        let mut config = pki.config(dir.path(), setup.policy, "127.0.0.1:0".parse().expect("addr"));
        config.non_tls_ports = setup
            .non_tls_ports
            .iter()
            .map(|port| std::num::NonZeroU16::new(*port).expect("port"))
            .collect();
        config.limits = Limits {
            first_record_timeout: Duration::from_secs(1),
            ..Limits::default()
        };
        let resolver = Names(
            setup
                .names
                .iter()
                .map(|(name, ip)| ((*name).to_owned(), ip.parse().expect("ip")))
                .collect(),
        );
        let audit = Arc::new(Recorder::default());
        let server = start_with(config, Arc::new(resolver), audit.clone())
            .await
            .expect("proxy starts");
        let address = server.local_addr().expect("addr");
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(server.run(shutdown.clone()));
        Self {
            address,
            audit,
            pki,
            shutdown,
            task,
            _dir: dir,
        }
    }

    /// Opens the mTLS leg to the proxy as `identity` (or with no certificate).
    pub async fn connect(&self, identity: Option<&ClientIdentity>) -> io::Result<TlsStream<TcpStream>> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut roots = RootCertStore::empty();
        for certificate in rustls_pemfile::certs(&mut self.pki.ca_pem().as_bytes()) {
            roots.add(certificate.expect("pem")).expect("root");
        }
        let builder = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(roots);
        let config = match identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.chain.clone(), identity.key.clone_key())
                .expect("client auth"),
            None => builder.with_no_client_auth(),
        };
        let tcp = TcpStream::connect(self.address).await?;
        let name = ServerName::try_from("localhost").expect("name");
        TlsConnector::from(Arc::new(config)).connect(name, tcp).await
    }

    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.task.await;
    }
}

/// A plain TCP upstream that echoes and remembers what it received.
pub struct Upstream {
    pub port: u16,
    pub received: Arc<Mutex<Vec<u8>>>,
}

impl Upstream {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let sink = Arc::clone(&sink);
                tokio::spawn(async move {
                    let mut buffer = [0u8; 4096];
                    while let Ok(read) = stream.read(&mut buffer).await {
                        if read == 0 {
                            return;
                        }
                        sink.lock().expect("lock").extend_from_slice(&buffer[..read]);
                        if stream.write_all(&buffer[..read]).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { port, received }
    }

    pub fn received(&self) -> Vec<u8> {
        self.received.lock().expect("lock").clone()
    }
}

/// The first flight of a real TLS client for `name` (a host name or an IP).
pub fn client_hello(name: &str) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    let server_name = ServerName::try_from(name.to_owned()).expect("name");
    let mut connection = rustls::ClientConnection::new(Arc::new(config), server_name).expect("client");
    let mut out = Vec::new();
    connection.write_tls(&mut out).expect("hello");
    out
}

/// Reads until the end of the HTTP head and returns it as text.
pub async fn read_head<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut head = Vec::new();
    tokio::time::timeout(WAIT, async {
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0u8; 1];
            let read = stream.read(&mut byte).await.expect("read head");
            assert_eq!(
                read,
                1,
                "connection closed inside the head: {:?}",
                String::from_utf8_lossy(&head)
            );
            head.push(byte[0]);
        }
    })
    .await
    .expect("head arrives");
    String::from_utf8(head).expect("utf8")
}

/// Reads until the peer closes, keeping whatever arrived even if the close was
/// not clean.
pub async fn read_to_close<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
    let mut received = Vec::new();
    let mut buffer = [0u8; 1024];
    let _ = tokio::time::timeout(WAIT, async {
        while let Ok(read) = stream.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            received.extend_from_slice(&buffer[..read]);
        }
    })
    .await;
    received
}

pub async fn read_exactly<S: tokio::io::AsyncRead + Unpin>(stream: &mut S, len: usize) -> Vec<u8> {
    let mut buffer = vec![0u8; len];
    tokio::time::timeout(WAIT, stream.read_exact(&mut buffer))
        .await
        .expect("bytes arrive")
        .expect("read");
    buffer
}
