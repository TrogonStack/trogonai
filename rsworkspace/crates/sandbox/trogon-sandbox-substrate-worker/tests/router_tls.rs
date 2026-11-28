use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::routing::get;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    SanType,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{self, RootCertStore, ServerConfig};
use trogon_sandbox_actor_agent::{
    AGENT_PROTOCOL_VERSION_HEADER, ActorId, ActorIdentity, GUEST_PROTOCOL_VERSION, IDENTITY_PATH,
};
use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_substrate_worker::{ActorAgent, ActorName, ActorTarget, Atespace, RouterEndpoint, RouterTlsConfig};
use trogon_std::env::InMemoryEnv;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SERVER_NAME: &str = "atenet-router.ate-system.svc";

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Authority {
    cert: Certificate,
    issuer: Issuer<'static, KeyPair>,
}

impl Authority {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(Vec::<String>::new())?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let cert = params.self_signed(&key)?;
        Ok(Self {
            cert,
            issuer: Issuer::new(params, key),
        })
    }

    fn client(&self) -> Result<ClientIdentity, Box<dyn std::error::Error>> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(Vec::<String>::new())?;
        params.subject_alt_names = vec![SanType::URI(
            "spiffe://cluster.local/ns/ate-system/sa/atenet-client".try_into()?,
        )];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        Ok(ClientIdentity {
            cert: params.signed_by(&key, &self.issuer)?,
            key,
        })
    }
}

struct ClientIdentity {
    cert: Certificate,
    key: KeyPair,
}

impl ClientIdentity {
    fn bundle(&self) -> String {
        format!("{}{}", self.key.serialize_pem(), self.cert.pem())
    }
}

struct Pki {
    serving: Authority,
    clients: Authority,
    server_config: Arc<ServerConfig>,
}

impl Pki {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let serving = Authority::new()?;
        let clients = Authority::new()?;
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(vec![SERVER_NAME.to_string()])?;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let cert = params.signed_by(&key, &serving.issuer)?;
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut roots = RootCertStore::empty();
        roots.add(clients.cert.der().clone())?;
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build()?;
        let server_config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )?;
        Ok(Self {
            serving,
            clients,
            server_config: Arc::new(server_config),
        })
    }
}

struct Credentials {
    directory: tempfile::TempDir,
    ca: PathBuf,
    identity: PathBuf,
}

impl Credentials {
    fn new(ca_pem: &str, identity: &ClientIdentity) -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let ca = directory.path().join("ca.pem");
        let bundle = directory.path().join("credential-bundle.pem");
        std::fs::write(&ca, ca_pem)?;
        std::fs::write(&bundle, identity.bundle())?;
        Ok(Self {
            directory,
            ca,
            identity: bundle,
        })
    }

    fn config(&self, server_name: &str) -> Result<RouterTlsConfig, Box<dyn std::error::Error>> {
        Ok(RouterTlsConfig::new(
            &self.ca,
            &self.identity,
            &self.identity,
            server_name,
        )?)
    }
}

struct RouterFixture {
    endpoint: RouterEndpoint,
    peers: Arc<Mutex<Vec<Vec<u8>>>>,
    connects: Arc<Mutex<Vec<String>>>,
    guest_calls: Arc<AtomicUsize>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for RouterFixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn target() -> Result<ActorTarget, Box<dyn std::error::Error>> {
    Ok(ActorTarget::new(
        Atespace::for_tenant(&TenantId::new("tenant-a")?)?,
        ActorName::for_lease(&LeaseId::new("01J0"))?,
        8110,
    ))
}

impl RouterFixture {
    async fn start(tls: Option<Arc<ServerConfig>>) -> Result<Self, Box<dyn std::error::Error>> {
        let guest_calls = Arc::new(AtomicUsize::new(0));
        let calls = guest_calls.clone();
        let actor_id = ActorId::new("native-fixture-actor")?;
        let guest = axum::Router::new().route(
            IDENTITY_PATH,
            get(move || {
                let calls = calls.clone();
                let actor_id = actor_id.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        [(AGENT_PROTOCOL_VERSION_HEADER, GUEST_PROTOCOL_VERSION.to_string())],
                        axum::Json(ActorIdentity { actor_id }),
                    )
                }
            }),
        );
        let guest_listener = TcpListener::bind("127.0.0.1:0").await?;
        let guest_address = guest_listener.local_addr()?;
        let guest_task = tokio::spawn(async move {
            let _ = axum::serve(guest_listener, guest).await;
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?.to_string().parse()?;
        let peers = Arc::new(Mutex::new(Vec::new()));
        let connects = Arc::new(Mutex::new(Vec::new()));
        let seen_peers = peers.clone();
        let seen_connects = connects.clone();
        let router_task = tokio::spawn(async move {
            let acceptor = tls.map(TlsAcceptor::from);
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let Ok((socket, _)) = connection else { break };
                        let acceptor = acceptor.clone();
                        let peers = seen_peers.clone();
                        let connects = seen_connects.clone();
                        connections.spawn(async move {
                            if let Some(acceptor) = acceptor {
                                let Ok(Ok(stream)) = tokio::time::timeout(
                                    Duration::from_secs(2), acceptor.accept(socket),
                                ).await else { return };
                                let Some(peer) = stream
                                    .get_ref()
                                    .1
                                    .peer_certificates()
                                    .and_then(|certificates| certificates.first())
                                    .map(|certificate| certificate.to_vec())
                                else {
                                    return;
                                };
                                locked(&peers).push(peer);
                                let _ = tunnel(stream, guest_address, connects).await;
                            } else {
                                let _ = tunnel(socket, guest_address, connects).await;
                            }
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Ok(Self {
            endpoint,
            peers,
            connects,
            guest_calls,
            tasks: vec![guest_task, router_task],
        })
    }

    fn agent(&self, config: RouterTlsConfig) -> Result<ActorAgent, Box<dyn std::error::Error>> {
        Ok(ActorAgent::new(self.endpoint.clone(), target()?, &InMemoryEnv::new())?.with_router_tls(config))
    }

    fn peer_certificates(&self) -> Vec<Vec<u8>> {
        locked(&self.peers).clone()
    }

    fn connect_count(&self) -> usize {
        locked(&self.connects).len()
    }

    fn guest_call_count(&self) -> usize {
        self.guest_calls.load(Ordering::SeqCst)
    }
}

async fn tunnel<S>(mut stream: S, guest: std::net::SocketAddr, connects: Arc<Mutex<Vec<String>>>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = tokio::time::timeout(Duration::from_secs(2), read_headers(&mut stream)).await??;
    let header = target()
        .map_err(|error| io::Error::other(error.to_string()))?
        .header_value();
    let first_line_ok = request
        .lines()
        .next()
        .is_some_and(|line| line.ends_with(":8110 HTTP/1.1"));
    if !request.starts_with("CONNECT ")
        || !first_line_ok
        || !request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("ate-target-actor: {header}")))
    {
        return Err(io::Error::other("unexpected CONNECT routing contract"));
    }
    locked(&connects).push(request);
    stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    let mut backend = TcpStream::connect(guest).await?;
    tokio::io::copy_bidirectional(&mut stream, &mut backend).await?;
    Ok(())
}

async fn read_headers<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<String> {
    let mut bytes = Vec::new();
    while bytes.len() < 16 * 1024 {
        bytes.push(stream.read_u8().await?);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes).map_err(io::Error::other);
        }
    }
    Err(io::Error::other("CONNECT headers exceeded the fixture limit"))
}

#[tokio::test]
async fn authenticated_tls_connect_reaches_the_guest_and_presents_client_identity() -> TestResult {
    let pki = Pki::new()?;
    let identity = pki.clients.client()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &identity)?;
    let router = RouterFixture::start(Some(pki.server_config.clone())).await?;
    let response = router.agent(files.config(SERVER_NAME)?)?.identity().await?;
    assert_eq!(response.actor_id.as_str(), "native-fixture-actor");
    assert_eq!(router.guest_call_count(), 1);
    assert_eq!(router.peer_certificates(), vec![identity.cert.der().to_vec()]);
    assert_eq!(router.connect_count(), 1);
    Ok(())
}

#[tokio::test]
async fn an_untrusted_router_ca_is_refused_before_guest_access() -> TestResult {
    let pki = Pki::new()?;
    let files = Credentials::new(&Authority::new()?.cert.pem(), &pki.clients.client()?)?;
    let router = RouterFixture::start(Some(pki.server_config)).await?;
    assert!(router.agent(files.config(SERVER_NAME)?)?.identity().await.is_err());
    assert_eq!(router.guest_call_count(), 0);
    assert_eq!(router.connect_count(), 0);
    Ok(())
}

#[tokio::test]
async fn the_router_certificate_must_match_the_explicit_server_name() -> TestResult {
    let pki = Pki::new()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &pki.clients.client()?)?;
    let router = RouterFixture::start(Some(pki.server_config)).await?;
    assert!(
        router
            .agent(files.config("wrong-router.ate-system.svc")?)?
            .identity()
            .await
            .is_err()
    );
    assert_eq!(router.guest_call_count(), 0);
    assert_eq!(router.connect_count(), 0);
    Ok(())
}

#[tokio::test]
async fn the_router_refuses_a_client_identity_outside_its_trust_domain() -> TestResult {
    let pki = Pki::new()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &Authority::new()?.client()?)?;
    let router = RouterFixture::start(Some(pki.server_config)).await?;
    assert!(router.agent(files.config(SERVER_NAME)?)?.identity().await.is_err());
    assert_eq!(router.guest_call_count(), 0);
    assert_eq!(router.connect_count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_missing_client_identity_never_downgrades_to_plaintext() -> TestResult {
    let pki = Pki::new()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &pki.clients.client()?)?;
    let router = RouterFixture::start(None).await?;
    let missing = files.directory.path().join("missing.pem");
    let config = RouterTlsConfig::new(&files.ca, &missing, &missing, SERVER_NAME)?;
    assert!(router.agent(config)?.identity().await.is_err());
    assert_eq!(router.guest_call_count(), 0);
    assert_eq!(router.connect_count(), 0);
    Ok(())
}

#[tokio::test]
async fn projected_trust_and_combined_identity_files_are_reloaded_per_connection() -> TestResult {
    let pki = Pki::new()?;
    let first = pki.clients.client()?;
    let second = pki.clients.client()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &first)?;
    let router = RouterFixture::start(Some(pki.server_config.clone())).await?;
    let agent = router.agent(files.config(SERVER_NAME)?)?;
    agent.identity().await?;
    std::fs::write(&files.identity, second.bundle())?;
    agent.identity().await?;
    assert_eq!(
        router.peer_certificates(),
        vec![first.cert.der().to_vec(), second.cert.der().to_vec()]
    );

    std::fs::write(&files.ca, Authority::new()?.cert.pem())?;
    assert!(agent.identity().await.is_err());
    std::fs::write(&files.ca, pki.serving.cert.pem())?;
    std::fs::write(&files.identity, "")?;
    assert!(agent.identity().await.is_err());
    assert_eq!(router.guest_call_count(), 2);
    Ok(())
}

#[test]
fn router_tls_configuration_requires_all_paths_and_a_valid_server_name() {
    for (ca, cert, key, name) in [
        ("", "client.pem", "client.pem", SERVER_NAME),
        ("ca.pem", "", "client.pem", SERVER_NAME),
        ("ca.pem", "client.pem", "", SERVER_NAME),
        ("ca.pem", "client.pem", "client.pem", ""),
        ("ca.pem", "client.pem", "client.pem", "https://router:8444"),
    ] {
        assert!(RouterTlsConfig::new(ca, cert, key, name).is_err());
    }
}

#[tokio::test]
async fn separate_client_certificate_and_private_key_files_are_supported() -> TestResult {
    let pki = Pki::new()?;
    let identity = pki.clients.client()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &identity)?;
    let key_path = files.directory.path().join("client-key.pem");
    std::fs::write(&files.identity, identity.cert.pem())?;
    std::fs::write(&key_path, identity.key.serialize_pem())?;
    let config = RouterTlsConfig::new(&files.ca, &files.identity, &key_path, SERVER_NAME)?;
    let router = RouterFixture::start(Some(pki.server_config)).await?;
    router.agent(config)?.identity().await?;
    assert_eq!(router.guest_call_count(), 1);
    assert_eq!(router.peer_certificates(), vec![identity.cert.der().to_vec()]);
    Ok(())
}

#[tokio::test]
async fn incomplete_or_mismatched_client_material_never_reaches_a_plaintext_guest() -> TestResult {
    let pki = Pki::new()?;
    let identity = pki.clients.client()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &identity)?;
    let router = RouterFixture::start(None).await?;
    let other_key = pki.clients.client()?.key.serialize_pem();
    let key_path = files.directory.path().join("different-key.pem");
    std::fs::write(&key_path, &other_key)?;
    let config = RouterTlsConfig::new(&files.ca, &files.identity, &key_path, SERVER_NAME)?;
    assert!(router.agent(config)?.identity().await.is_err());

    std::fs::write(&files.identity, identity.cert.pem())?;
    assert!(router.agent(files.config(SERVER_NAME)?)?.identity().await.is_err());
    std::fs::write(&files.identity, identity.key.serialize_pem())?;
    assert!(router.agent(files.config(SERVER_NAME)?)?.identity().await.is_err());
    assert_eq!(router.guest_call_count(), 0);
    assert_eq!(router.connect_count(), 0);
    Ok(())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "asserts the error message never echoes CA material"
    )
)]
#[tokio::test]
async fn malformed_or_oversized_ca_material_is_refused_without_plaintext_fallback() -> TestResult {
    let pki = Pki::new()?;
    let files = Credentials::new(&pki.serving.cert.pem(), &pki.clients.client()?)?;
    let router = RouterFixture::start(None).await?;
    let agent = router.agent(files.config(SERVER_NAME)?)?;
    for bytes in [
        Vec::new(),
        b"fixture-invalid-CA-material".to_vec(),
        vec![b'A'; 1024 * 1024 + 1],
    ] {
        std::fs::write(&files.ca, bytes)?;
        let error = agent.identity().await.err().ok_or("a bad CA must be refused")?;
        assert!(!error.to_string().contains("fixture-invalid-CA-material"));
    }
    assert_eq!(router.guest_call_count(), 0);
    assert_eq!(router.connect_count(), 0);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn projected_volume_symlink_rotation_picks_up_the_new_client_identity() -> TestResult {
    let pki = Pki::new()?;
    let first = pki.clients.client()?;
    let second = pki.clients.client()?;
    let directory = tempfile::tempdir()?;
    for (name, identity) in [("revision-one", &first), ("revision-two", &second)] {
        let revision = directory.path().join(name);
        std::fs::create_dir(&revision)?;
        std::fs::write(revision.join("ca.pem"), pki.serving.cert.pem())?;
        std::fs::write(revision.join("identity.pem"), identity.bundle())?;
    }
    std::os::unix::fs::symlink("revision-one", directory.path().join("..data"))?;
    std::os::unix::fs::symlink("..data/ca.pem", directory.path().join("ca.pem"))?;
    std::os::unix::fs::symlink("..data/identity.pem", directory.path().join("identity.pem"))?;
    let ca = directory.path().join("ca.pem");
    let identity = directory.path().join("identity.pem");
    let config = RouterTlsConfig::new(&ca, &identity, &identity, SERVER_NAME)?;
    let router = RouterFixture::start(Some(pki.server_config)).await?;
    let agent = router.agent(config)?;
    agent.identity().await?;
    std::os::unix::fs::symlink("revision-two", directory.path().join("..data-next"))?;
    std::fs::rename(directory.path().join("..data-next"), directory.path().join("..data"))?;
    agent.identity().await?;
    assert_eq!(
        router.peer_certificates(),
        vec![first.cert.der().to_vec(), second.cert.der().to_vec()]
    );
    Ok(())
}
