use super::*;
use crate::held_writes::{HeldWrites, WAIT, expect_bytes};
use crate::oidc_signer::validation_for_audience;
use crate::resolve::VettedAddr;
use crate::tls_ca::{PermittedDomains, install_default_crypto_provider};
use rcgen::string::Ia5String;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::duplex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, ForwardUrl, HeaderName,
};

/// Deadlines that leave room for a loaded machine: a handshake or a first
/// request may take many seconds when certificates are minted in an
/// unoptimized build.
pub(crate) fn patient_deadlines() -> TunnelDeadlines {
    TunnelDeadlines {
        handshake: Duration::from_secs(300),
        first_request: Duration::from_secs(300),
        ..TunnelDeadlines::default()
    }
}

/// [`super::terminate_and_forward`] with [`patient_deadlines`].
#[allow(clippy::too_many_arguments)]
pub(crate) async fn terminate_and_forward<S>(
    client_stream: PrefixedStream<S>,
    sni_host: &str,
    ca: &SandboxCa,
    upstream: VettedEndpoint,
    marking: UpstreamMarking,
    upstream_tls_config: Arc<rustls::ClientConfig>,
    credential_injection: Option<CredentialInjectionContext<'_>>,
    forward: Option<ForwardContext<'_>>,
) -> Result<(u64, u64), TlsInterceptError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    terminate_and_forward_with_secrets(
        client_stream,
        sni_host,
        ca,
        upstream,
        marking,
        upstream_tls_config,
        credential_injection,
        forward,
        None,
        patient_deadlines(),
    )
    .await
}

pub(crate) type TestError = Box<dyn std::error::Error>;
type TaskError = Box<dyn std::error::Error + Send + Sync>;

/// Flattens the outcome of a spawned task that reports a failure of its own.
fn joined<T>(outcome: Result<Result<T, TaskError>, tokio::task::JoinError>) -> Result<T, TestError> {
    outcome?.map_err(|error| error.to_string().into())
}

pub(crate) fn vetted(addr: std::net::SocketAddr) -> VettedEndpoint {
    VettedAddr::assume_vetted(addr.ip()).with_port(addr.port())
}

pub(crate) struct TestUpstreamCa {
    pub(crate) pem: String,
    pub(crate) cert: rustls::pki_types::CertificateDer<'static>,
    pub(crate) key: rustls::pki_types::PrivateKeyDer<'static>,
}

pub(crate) fn build_test_upstream_ca_and_leaf(hostname: &str) -> Result<TestUpstreamCa, TestError> {
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Test Upstream CA");
        dn
    };
    ca_params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params.self_signed(&ca_key)?;
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf_key = KeyPair::generate()?;
    let mut leaf_params = CertificateParams::new(vec![hostname.to_string()])?;
    leaf_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, hostname);
        dn
    };
    leaf_params.subject_alt_names = vec![SanType::DnsName(Ia5String::try_from(hostname.to_string())?)];
    let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer)?;

    Ok(TestUpstreamCa {
        pem: ca_cert.pem(),
        cert: rustls::pki_types::CertificateDer::from(leaf_cert.der().to_vec()),
        key: rustls::pki_types::PrivateKeyDer::try_from(leaf_key.serialize_der())?,
    })
}

/// Same as [`build_test_upstream_ca_and_leaf`] but issues the leaf for an
/// IP address SAN (`SanType::IpAddress`) rather than a DNS name SAN,
/// since [`connect_forward_target`] dials `forward_url`'s host over raw
/// TCP and a loopback IP is the only "host" a test can reliably bind to
/// without depending on real DNS.
pub(crate) fn build_test_forward_target_ca_and_leaf(ip: std::net::IpAddr) -> Result<TestUpstreamCa, TestError> {
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Test Forward Target CA");
        dn
    };
    ca_params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params.self_signed(&ca_key)?;
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf_key = KeyPair::generate()?;
    let mut leaf_params = CertificateParams::default();
    leaf_params.distinguished_name = {
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, ip.to_string());
        dn
    };
    leaf_params.subject_alt_names = vec![SanType::IpAddress(ip)];
    let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer)?;

    Ok(TestUpstreamCa {
        pem: ca_cert.pem(),
        cert: rustls::pki_types::CertificateDer::from(leaf_cert.der().to_vec()),
        key: rustls::pki_types::PrivateKeyDer::try_from(leaf_key.serialize_der())?,
    })
}

pub(crate) fn root_store_from_pem(pem: &str) -> Result<rustls::RootCertStore, TestError> {
    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(pem.as_bytes().to_vec());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    Ok(root_store)
}

const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

#[tokio::test]
async fn terminate_and_forward_round_trips_bytes_through_mock_upstream() -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();

    let hostname = "api.example.com";
    let upstream_ca = build_test_upstream_ca_and_leaf(hostname)?;

    let upstream_server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![upstream_ca.cert.clone()], upstream_ca.key)?,
    );
    let upstream_acceptor = TlsAcceptor::from(upstream_server_config);

    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;

    let upstream_task = tokio::spawn(async move {
        let (tcp, _) = upstream_listener.accept().await?;
        let mut tls = upstream_acceptor.accept(tcp).await?;
        let mut buf = [0u8; 1024];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = tls.read(&mut buf).await?;
            assert_ne!(n, 0);
            seen.extend_from_slice(&buf[..n]);
        }
        tls.write_all(RESPONSE).await?;
        tls.shutdown().await?;
        Ok::<(), std::io::Error>(())
    });

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let sandbox_root_pem = sandbox_ca.root_cert_pem();

    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&upstream_ca.pem)?)
            .with_no_client_auth(),
    );

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let proxy_task = tokio::spawn(async move {
        let (mut client_tcp, _) = proxy_listener.accept().await?;
        let (sni, prefix) = crate::proxy::sniff_client_hello(&mut client_tcp).await?;
        assert_eq!(sni.as_deref(), Some(hostname));

        let client_stream = PrefixedStream::new(client_tcp, prefix);
        terminate_and_forward(
            client_stream,
            hostname,
            &sandbox_ca,
            vetted(upstream_addr),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            None,
            None,
        )
        .await
        .map_err(TaskError::from)
    });

    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(sandbox_root_pem.into_bytes());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );
    let connector = TlsConnector::from(client_config);

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;

    let request = format!(
        "GET / HTTP/1.1\r\nHost: {hostname}:{}\r\nConnection: close\r\n\r\n",
        upstream_addr.port()
    );
    client_tls.write_all(request.as_bytes()).await?;
    let mut echoed = Vec::new();
    let _ = client_tls.read_to_end(&mut echoed).await;
    assert!(echoed.ends_with(b"\r\n\r\nok"), "{echoed:?}");

    let _ = client_tls.shutdown().await;

    let (bytes_client_to_upstream, bytes_upstream_to_client) = joined(proxy_task.await)?;
    let connection_header = "Connection: close\r\n".len() as u64;
    assert!(bytes_client_to_upstream >= request.len() as u64 - connection_header);
    assert!(bytes_upstream_to_client >= echoed.len() as u64);

    upstream_task.abort();
    Ok(())
}

#[tokio::test]
async fn terminate_and_forward_rejects_upstream_signed_by_untrusted_ca() -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();

    let hostname = "api.example.com";
    // The upstream server's real certificate, signed by `upstream_ca`.
    let upstream_ca = build_test_upstream_ca_and_leaf(hostname)?;
    // A second, unrelated CA: `upstream_tls_config` below is configured to
    // trust only this one, standing in for "the real system root store
    // doesn't recognize this upstream," e.g. a MITM or a misconfigured
    // destination presenting a cert no legitimate CA issued.
    let untrusted_ca = build_test_upstream_ca_and_leaf(hostname)?;

    let upstream_server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![upstream_ca.cert.clone()], upstream_ca.key)?,
    );
    let upstream_acceptor = TlsAcceptor::from(upstream_server_config);

    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;

    let upstream_task = tokio::spawn(async move {
        let (tcp, _) = upstream_listener.accept().await?;
        // The handshake itself must fail before any application data is
        // exchanged, so accept() erroring out is the expected outcome
        // here, not a bug in the test.
        let _ = upstream_acceptor.accept(tcp).await;
        Ok::<(), std::io::Error>(())
    });

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let sandbox_root_pem = sandbox_ca.root_cert_pem();

    // Trusts only `untrusted_ca`, never `upstream_ca`, so validating the
    // real upstream's leaf must fail.
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&untrusted_ca.pem)?)
            .with_no_client_auth(),
    );

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let proxy_task = tokio::spawn(async move {
        let (mut client_tcp, _) = proxy_listener.accept().await?;
        let (sni, prefix) = crate::proxy::sniff_client_hello(&mut client_tcp).await?;
        assert_eq!(sni.as_deref(), Some(hostname));

        let client_stream = PrefixedStream::new(client_tcp, prefix);
        let result = terminate_and_forward(
            client_stream,
            hostname,
            &sandbox_ca,
            vetted(upstream_addr),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            None,
            None,
        )
        .await;
        Ok::<_, std::io::Error>(result)
    });

    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(sandbox_root_pem.into_bytes());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );
    let connector = TlsConnector::from(client_config);

    // The client-facing handshake (against the sandbox CA's freshly
    // minted leaf) succeeds normally: only the *upstream* leg is
    // untrusted, and the proxy dials it only once a request arrives, then
    // answers that request with a 502.
    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;
    let request = format!("GET / HTTP/1.1\r\nHost: {hostname}:{}\r\n\r\n", upstream_addr.port());
    for _ in 0..2 {
        client_tls.write_all(request.as_bytes()).await?;
        let mut answered = Vec::new();
        let mut chunk = [0u8; 512];
        while !answered.ends_with(b"Bad Gateway\r\n0\r\n\r\n") {
            let n = timeout(Duration::from_secs(5), client_tls.read(&mut chunk)).await??;
            assert_ne!(n, 0, "the tunnel stays open after a 502: {answered:?}");
            answered.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(
            crate::framed_head::undated(&String::from_utf8_lossy(&answered)),
            "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nTransfer-Encoding: chunked\r\n\r\nb\r\nBad Gateway\r\n0\r\n\r\n"
        );
    }
    client_tls.shutdown().await?;

    let result = timeout(Duration::from_secs(5), proxy_task).await???;
    assert!(result.is_ok(), "{result:?}");

    upstream_task.abort();
    Ok(())
}

const KEEP_ALIVE_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
const KEPT_ALIVE_ANSWER: &str =
    "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nDate: <DATE>\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\n\r\nok";
const IMF_FIXDATE_LEN: usize = "Tue, 14 Nov 2023 22:13:20 GMT".len();

struct LazyFixture {
    connector: TlsConnector,
    proxy_addr: SocketAddr,
    upstream_port: u16,
    accepted: Arc<std::sync::atomic::AtomicUsize>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl LazyFixture {
    const HOST: &'static str = "api.example.com";

    async fn start() -> Result<Self, TestError> {
        install_default_crypto_provider();
        let upstream_ca = build_test_upstream_ca_and_leaf(Self::HOST)?;
        let acceptor = TlsAcceptor::from(Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![upstream_ca.cert.clone()], upstream_ca.key)?,
        ));
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
        let upstream_addr = upstream_listener.local_addr()?;
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&accepted);
        let upstream_task = tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = upstream_listener.accept().await else {
                    return;
                };
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 1024];
                    let mut seen = Vec::new();
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = tls.write_all(KEEP_ALIVE_RESPONSE).await;
                    let _ = tls.shutdown().await;
                });
            }
        });

        let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
        let sandbox_root_pem = sandbox_ca.root_cert_pem();
        let upstream_tls_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store_from_pem(&upstream_ca.pem)?)
                .with_no_client_auth(),
        );
        let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy_addr = proxy_listener.local_addr()?;
        let proxy_task = tokio::spawn(async move {
            let Ok((mut client_tcp, _)) = proxy_listener.accept().await else {
                return;
            };
            let Ok((_, prefix)) = crate::proxy::sniff_client_hello(&mut client_tcp).await else {
                return;
            };
            let _ = terminate_and_forward(
                PrefixedStream::new(client_tcp, prefix),
                Self::HOST,
                &sandbox_ca,
                vetted(upstream_addr),
                UpstreamMarking::NotNeeded,
                upstream_tls_config,
                None,
                None,
            )
            .await;
        });

        let connector = TlsConnector::from(Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store_from_pem(&sandbox_root_pem)?)
                .with_no_client_auth(),
        ));
        Ok(Self {
            connector,
            proxy_addr,
            upstream_port: upstream_addr.port(),
            accepted,
            tasks: vec![upstream_task, proxy_task],
        })
    }

    async fn client(&self) -> Result<tokio_rustls::client::TlsStream<TcpStream>, TestError> {
        let tcp = TcpStream::connect(self.proxy_addr).await?;
        let name = rustls::pki_types::ServerName::try_from(Self::HOST)?;
        Ok(self.connector.connect(name, tcp).await?)
    }

    async fn get(&self, tls: &mut tokio_rustls::client::TlsStream<TcpStream>) -> Result<String, TestError> {
        let request = format!("GET / HTTP/1.1\r\nHost: {}:{}\r\n\r\n", Self::HOST, self.upstream_port);
        tls.write_all(request.as_bytes()).await?;
        let mut answered = vec![0u8; KEPT_ALIVE_ANSWER.len() - "<DATE>".len() + IMF_FIXDATE_LEN];
        timeout(Duration::from_secs(5), tls.read_exact(&mut answered)).await??;
        Ok(crate::framed_head::undated(&String::from_utf8_lossy(&answered)))
    }

    fn upstream_connections(&self) -> usize {
        self.accepted.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for LazyFixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[tokio::test]
async fn the_upstream_is_not_dialed_until_a_request_arrives() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = LazyFixture::start().await?;
    let mut client = fixture.client().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.upstream_connections(), 0);

    assert_eq!(fixture.get(&mut client).await?, KEPT_ALIVE_ANSWER);
    assert_eq!(fixture.upstream_connections(), 1);
    Ok(())
}

#[tokio::test]
async fn the_upstream_is_dialed_again_when_it_closed_while_idle() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = LazyFixture::start().await?;
    let mut client = fixture.client().await?;
    assert_eq!(fixture.get(&mut client).await?, KEPT_ALIVE_ANSWER);
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(fixture.get(&mut client).await?, KEPT_ALIVE_ANSWER);
    assert_eq!(fixture.upstream_connections(), 2);
    Ok(())
}

fn test_transform(match_rules: Vec<trogon_sandbox_contract::MatchRule>) -> Result<CredentialTransform, TestError> {
    Ok(CredentialTransform {
        match_rules,
        credential: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: trogon_sandbox_contract::PullSecretId::new("test-secret"),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
        injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
    })
}

/// Spawns a plaintext (post-TLS-termination on the proxy's side) mock
/// HTTP/1.1 upstream on `upstream_listener` that accepts a TLS
/// connection, reads exactly one request, records the exact bytes it
/// received, and replies with a fixed 200 response, then closes.
async fn spawn_recording_http_upstream(
    upstream_listener: TcpListener,
    upstream_ca: &TestUpstreamCa,
) -> Result<(SocketAddr, tokio::task::JoinHandle<Vec<u8>>), TestError> {
    let upstream_addr = upstream_listener.local_addr()?;
    let upstream_server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![upstream_ca.cert.clone()], upstream_ca.key.clone_key())?,
    );
    let upstream_acceptor = TlsAcceptor::from(upstream_server_config);

    let upstream_task = tokio::spawn(async move {
        let Ok((tcp, _)) = upstream_listener.accept().await else {
            return Vec::new();
        };
        let Ok(mut tls) = upstream_acceptor.accept(tcp).await else {
            return Vec::new();
        };

        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = tls.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        if !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return buf;
        }

        let body = b"ok";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = tls.write_all(response.as_bytes()).await;
        let _ = tls.write_all(body).await;
        let _ = tls.shutdown().await;

        buf
    });

    Ok((upstream_addr, upstream_task))
}

/// Reads exactly one HTTP/1.1 response (status line + headers + a
/// `Content-Length` body) off `client_tls`. The proxy keeps the
/// connection open for further keep-alive requests after relaying a
/// response, so callers must read a bounded amount rather than
/// `read_to_end` (which would block waiting for the proxy to close).
async fn read_fixed_http_response(
    client_tls: &mut (impl AsyncRead + AsyncWrite + Unpin),
) -> Result<Vec<u8>, TestError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        let n = client_tls.read(&mut chunk).await?;
        assert!(n > 0, "connection closed before response headers arrived");
        buf.extend_from_slice(&chunk[..n]);
    };

    let header_text = std::str::from_utf8(&buf[..header_end])?;
    let content_length: usize = match header_text
        .split("\r\n")
        .find_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
    {
        Some((_, value)) => value.trim().parse()?,
        None => 0,
    };

    while buf.len() < header_end + content_length {
        let n = client_tls.read(&mut chunk).await?;
        assert!(n > 0, "connection closed before response body arrived");
        buf.extend_from_slice(&chunk[..n]);
    }

    Ok(buf)
}

#[tokio::test]
async fn terminate_and_forward_injects_credential_into_matched_request() -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();

    let hostname = "registry.example.com";
    let upstream_ca = build_test_upstream_ca_and_leaf(hostname)?;

    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let (upstream_addr, upstream_task) = spawn_recording_http_upstream(upstream_listener, &upstream_ca).await?;

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let sandbox_root_pem = sandbox_ca.root_cert_pem();

    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&upstream_ca.pem)?)
            .with_no_client_auth(),
    );

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let transform = test_transform(Vec::new())?;
    let credential_source =
        crate::credential_source::tests::FakeCredentialSource::new().with_token("test-secret", "s3cr3t-token");

    let proxy_task = tokio::spawn(async move {
        let (mut client_tcp, _) = proxy_listener.accept().await?;
        let (sni, prefix) = crate::proxy::sniff_client_hello(&mut client_tcp).await?;
        assert_eq!(sni.as_deref(), Some(hostname));

        let client_stream = PrefixedStream::new(client_tcp, prefix);
        terminate_and_forward(
            client_stream,
            hostname,
            &sandbox_ca,
            vetted(upstream_addr),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            Some(CredentialInjectionContext {
                transform: &transform,
                credential_source: &credential_source,
            }),
            None,
        )
        .await
        .map_err(TaskError::from)
    });

    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(sandbox_root_pem.into_bytes());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );
    let connector = TlsConnector::from(client_config);

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;

    client_tls
        .write_all(
            format!(
                "GET /v2/repo/manifests/latest HTTP/1.1\r\nHost: registry.example.com:{}\r\n\r\n",
                upstream_addr.port()
            )
            .as_bytes(),
        )
        .await?;

    let response = read_fixed_http_response(&mut client_tls).await?;
    assert!(
        response.starts_with(b"HTTP/1.1 200 OK"),
        "expected 200 response, got {response:?}"
    );
    client_tls.shutdown().await?;

    joined(proxy_task.await)?;
    let received = upstream_task.await?;
    let received_text = String::from_utf8(received)?;
    assert!(
        received_text.contains("authorization: s3cr3t-token") || received_text.contains("Authorization: s3cr3t-token"),
        "expected injected Authorization header in request upstream saw, got: {received_text}"
    );
    Ok(())
}

#[tokio::test]
async fn terminate_and_forward_does_not_inject_credential_into_unmatched_request()
-> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();

    let hostname = "registry.example.com";
    let upstream_ca = build_test_upstream_ca_and_leaf(hostname)?;

    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let (upstream_addr, upstream_task) = spawn_recording_http_upstream(upstream_listener, &upstream_ca).await?;

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let sandbox_root_pem = sandbox_ca.root_cert_pem();

    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&upstream_ca.pem)?)
            .with_no_client_auth(),
    );

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    // Only matches requests to `/v2/*`; the request sent below targets
    // `/health`, so it must reach upstream with no credential injected.
    let transform = test_transform(vec![trogon_sandbox_contract::MatchRule {
        path: Some(trogon_sandbox_contract::StringMatcher::StartsWith("/v2/".to_string())),
        methods: Vec::new(),
        query_string: Vec::new(),
        headers: Vec::new(),
    }])?;
    let credential_source =
        crate::credential_source::tests::FakeCredentialSource::new().with_token("test-secret", "s3cr3t-token");

    let proxy_task = tokio::spawn(async move {
        let (mut client_tcp, _) = proxy_listener.accept().await?;
        let (sni, prefix) = crate::proxy::sniff_client_hello(&mut client_tcp).await?;
        assert_eq!(sni.as_deref(), Some(hostname));

        let client_stream = PrefixedStream::new(client_tcp, prefix);
        terminate_and_forward(
            client_stream,
            hostname,
            &sandbox_ca,
            vetted(upstream_addr),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            Some(CredentialInjectionContext {
                transform: &transform,
                credential_source: &credential_source,
            }),
            None,
        )
        .await
        .map_err(TaskError::from)
    });

    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(sandbox_root_pem.into_bytes());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );
    let connector = TlsConnector::from(client_config);

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;

    client_tls
        .write_all(
            format!(
                "GET /health HTTP/1.1\r\nHost: registry.example.com:{}\r\n\r\n",
                upstream_addr.port()
            )
            .as_bytes(),
        )
        .await?;

    let response = read_fixed_http_response(&mut client_tls).await?;
    assert!(
        response.starts_with(b"HTTP/1.1 200 OK"),
        "expected 200 response, got {response:?}"
    );
    client_tls.shutdown().await?;

    joined(proxy_task.await)?;
    let received = upstream_task.await?;
    let received_text = String::from_utf8(received)?;
    assert!(
        !received_text.to_ascii_lowercase().contains("s3cr3t-token"),
        "unmatched request must not carry the injected credential, got: {received_text}"
    );
    Ok(())
}

fn test_request_forward(
    forward_url: &str,
    match_rules: Vec<trogon_sandbox_contract::MatchRule>,
) -> Result<RequestForward, TestError> {
    Ok(RequestForward {
        forward_url: ForwardUrl::new(forward_url)?,
        match_rules,
        ttl_seconds: 60,
    })
}

#[tokio::test]
async fn an_unmatched_request_head_is_flushed_to_the_original_upstream_before_the_response_is_awaited()
-> Result<(), TestError> {
    install_default_crypto_provider();
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream) = duplex(64 * 1024);
    let forward_rule = test_request_forward(
        "https://127.0.0.1:1/hook",
        vec![trogon_sandbox_contract::MatchRule {
            path: Some(trogon_sandbox_contract::StringMatcher::Exact("/hook".to_string())),
            methods: Vec::new(),
            query_string: Vec::new(),
            headers: Vec::new(),
        }],
    )?;
    let match_rules = CompiledMatchRules::compile(&forward_rule.match_rules)?;
    let signer = OidcSigner::generate()?;
    let subject = SandboxId::new("sandbox-abc123")?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1".to_string());
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    );
    let proxy = tokio::spawn(async move {
        run_forward_intercept(
            proxy_client,
            UpstreamLeg::connected(HeldWrites::new(proxy_upstream)),
            "app.example.com",
            upstream_tls_config,
            ForwardContext {
                forward: &forward_rule,
                signer: &signer,
                issuer: "sandbox-egress-proxy",
                subject: &subject,
                tenant_id: &tenant_id,
                lease_id: &lease_id,
                original_scheme: "https",
                original_port: 443,
                marking: UpstreamMarking::NotNeeded,
            },
            &match_rules,
            &RequestGuard::new(TlsTarget::new("app.example.com", 443)),
        )
        .await
    });
    let request = b"GET /other HTTP/1.1\r\nHost: app.example.com\r\n\r\n";
    client.write_all(request).await?;
    expect_bytes(&mut upstream, request, "the request head").await;
    upstream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        .await?;
    client.shutdown().await?;
    timeout(WAIT, proxy).await???;
    Ok(())
}

#[tokio::test]
async fn terminate_and_forward_routes_matched_request_to_forward_url_with_oidc_token()
-> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();

    let hostname = "app.example.com";
    let original_ca = build_test_upstream_ca_and_leaf(hostname)?;
    let forward_ip: std::net::IpAddr = "127.0.0.1".parse()?;
    let forward_ca = build_test_forward_target_ca_and_leaf(forward_ip)?;

    let original_listener = TcpListener::bind("127.0.0.1:0").await?;
    let (original_addr, mut original_task) = spawn_recording_http_upstream(original_listener, &original_ca).await?;

    let forward_listener = TcpListener::bind("127.0.0.1:0").await?;
    let forward_port = forward_listener.local_addr()?.port();
    let (_forward_addr, forward_task) = spawn_recording_http_upstream(forward_listener, &forward_ca).await?;

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let sandbox_root_pem = sandbox_ca.root_cert_pem();

    let mut trusted_roots = root_store_from_pem(&original_ca.pem)?;
    for cert in rustls_pemfile::certs(&mut std::io::Cursor::new(forward_ca.pem.as_bytes().to_vec())) {
        trusted_roots.add(cert?)?;
    }
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(trusted_roots)
            .with_no_client_auth(),
    );

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let forward_url = format!("https://127.0.0.1:{forward_port}/hook");
    let forward_rule = test_request_forward(&forward_url, Vec::new())?;
    let signer = OidcSigner::generate()?;
    let subject = SandboxId::new("sandbox-abc123")?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1".to_string());

    let decoding_key = signer.decoding_key();
    let expected_tenant_id = tenant_id.clone();
    let expected_lease_id = lease_id.clone();
    let expected_subject = subject.clone();

    let proxy_task = tokio::spawn(async move {
        let (mut client_tcp, _) = proxy_listener.accept().await?;
        let (sni, prefix) = crate::proxy::sniff_client_hello(&mut client_tcp).await?;
        assert_eq!(sni.as_deref(), Some(hostname));

        let client_stream = PrefixedStream::new(client_tcp, prefix);
        terminate_and_forward(
            client_stream,
            hostname,
            &sandbox_ca,
            vetted(original_addr),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            None,
            Some(ForwardContext {
                forward: &forward_rule,
                signer: &signer,
                issuer: "sandbox-egress-proxy",
                subject: &subject,
                tenant_id: &tenant_id,
                lease_id: &lease_id,
                original_scheme: "https",
                original_port: 443,
                marking: UpstreamMarking::NotNeeded,
            }),
        )
        .await
        .map_err(TaskError::from)
    });

    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(sandbox_root_pem.into_bytes());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );
    let connector = TlsConnector::from(client_config);

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;

    client_tls
        .write_all(
            format!(
                "GET /webhook HTTP/1.1\r\nHost: app.example.com:{}\r\n\r\n",
                original_addr.port()
            )
            .as_bytes(),
        )
        .await?;

    let response = read_fixed_http_response(&mut client_tls).await?;
    assert!(
        response.starts_with(b"HTTP/1.1 200 OK"),
        "expected 200 response, got {response:?}"
    );
    client_tls.shutdown().await?;

    joined(proxy_task.await)?;

    // The original destination must never see a matched request at all.
    match tokio::time::timeout(std::time::Duration::from_millis(500), &mut original_task).await {
        Ok(saw) => {
            let saw = saw?;
            assert!(saw.is_empty(), "{saw:?}");
        }
        Err(_) => original_task.abort(),
    }

    let forwarded = forward_task.await?;
    let forwarded_text = String::from_utf8(forwarded)?;

    assert!(
        forwarded_text.contains("trogon-forwarded-host: app.example.com")
            || forwarded_text.contains("Trogon-Forwarded-Host: app.example.com"),
        "expected trogon-forwarded-host header, got: {forwarded_text}"
    );
    assert!(
        forwarded_text
            .to_ascii_lowercase()
            .contains("trogon-forwarded-scheme: https"),
        "expected trogon-forwarded-scheme header, got: {forwarded_text}"
    );
    assert!(
        forwarded_text
            .to_ascii_lowercase()
            .contains("trogon-forwarded-port: 443"),
        "expected trogon-forwarded-port header, got: {forwarded_text}"
    );
    assert!(
        forwarded_text
            .to_ascii_lowercase()
            .contains("trogon-forwarded-path: /webhook"),
        "expected trogon-forwarded-path header, got: {forwarded_text}"
    );

    let token = forwarded_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("trogon-sandbox-oidc-token")
                .then(|| value.trim().to_string())
        })
        .ok_or("expected a value")?;

    let validation = validation_for_audience(&format!("https://127.0.0.1:{forward_port}/hook"));
    let decoded = jsonwebtoken::decode::<trogon_sandbox_contract::OidcClaims>(&token, &decoding_key, &validation)?;
    assert_eq!(decoded.claims.sub, expected_subject);
    assert_eq!(decoded.claims.tenant_id, expected_tenant_id);
    assert_eq!(decoded.claims.lease_id, expected_lease_id);
    assert_eq!(decoded.claims.exp - decoded.claims.iat, 60);
    Ok(())
}

#[tokio::test]
async fn terminate_and_forward_routes_unmatched_request_to_original_destination()
-> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();

    let hostname = "app.example.com";
    let original_ca = build_test_upstream_ca_and_leaf(hostname)?;
    let forward_ip: std::net::IpAddr = "127.0.0.1".parse()?;
    let forward_ca = build_test_forward_target_ca_and_leaf(forward_ip)?;

    let original_listener = TcpListener::bind("127.0.0.1:0").await?;
    let (original_addr, original_task) = spawn_recording_http_upstream(original_listener, &original_ca).await?;

    // Bound but deliberately never driven to accept: an unmatched
    // request must never reach this forward target at all.
    let forward_listener = TcpListener::bind("127.0.0.1:0").await?;
    let forward_port = forward_listener.local_addr()?.port();
    drop(forward_listener);

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let sandbox_root_pem = sandbox_ca.root_cert_pem();

    let mut trusted_roots = root_store_from_pem(&original_ca.pem)?;
    for cert in rustls_pemfile::certs(&mut std::io::Cursor::new(forward_ca.pem.as_bytes().to_vec())) {
        trusted_roots.add(cert?)?;
    }
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(trusted_roots)
            .with_no_client_auth(),
    );

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    // Only matches `/webhook`; the request sent below targets `/health`,
    // so it must reach the original destination, not `forward_url`.
    let forward_url = format!("https://127.0.0.1:{forward_port}/hook");
    let forward_rule = test_request_forward(
        &forward_url,
        vec![trogon_sandbox_contract::MatchRule {
            path: Some(trogon_sandbox_contract::StringMatcher::Exact("/webhook".to_string())),
            methods: Vec::new(),
            query_string: Vec::new(),
            headers: Vec::new(),
        }],
    )?;
    let signer = OidcSigner::generate()?;
    let subject = SandboxId::new("sandbox-abc123")?;
    let tenant_id = TenantId::new("tenant-1")?;
    let lease_id = LeaseId::new("lease-1".to_string());

    let proxy_task = tokio::spawn(async move {
        let (mut client_tcp, _) = proxy_listener.accept().await?;
        let (sni, prefix) = crate::proxy::sniff_client_hello(&mut client_tcp).await?;
        assert_eq!(sni.as_deref(), Some(hostname));

        let client_stream = PrefixedStream::new(client_tcp, prefix);
        terminate_and_forward(
            client_stream,
            hostname,
            &sandbox_ca,
            vetted(original_addr),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            None,
            Some(ForwardContext {
                forward: &forward_rule,
                signer: &signer,
                issuer: "sandbox-egress-proxy",
                subject: &subject,
                tenant_id: &tenant_id,
                lease_id: &lease_id,
                original_scheme: "https",
                original_port: 443,
                marking: UpstreamMarking::NotNeeded,
            }),
        )
        .await
        .map_err(TaskError::from)
    });

    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(sandbox_root_pem.into_bytes());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    let client_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );
    let connector = TlsConnector::from(client_config);

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;

    client_tls
        .write_all(
            format!(
                "GET /health HTTP/1.1\r\nHost: app.example.com:{}\r\n\r\n",
                original_addr.port()
            )
            .as_bytes(),
        )
        .await?;

    let response = read_fixed_http_response(&mut client_tls).await?;
    assert!(
        response.starts_with(b"HTTP/1.1 200 OK"),
        "expected 200 response, got {response:?}"
    );
    client_tls.shutdown().await?;

    joined(proxy_task.await)?;
    let received = original_task.await?;
    let received_text = String::from_utf8(received)?;
    assert!(
        received_text.starts_with("GET /health HTTP/1.1"),
        "expected unmatched request to reach the original destination, got: {received_text}"
    );
    assert!(
        !received_text.to_ascii_lowercase().contains("trogon-forwarded-host"),
        "unmatched request must not carry forwarded headers, got: {received_text}"
    );
    assert!(
        !received_text.to_ascii_lowercase().contains("trogon-sandbox-oidc-token"),
        "unmatched request must not carry an OIDC token, got: {received_text}"
    );
    Ok(())
}

fn handshake_probe() -> Result<(SandboxCa, Arc<rustls::ClientConfig>, VettedEndpoint), TestError> {
    install_default_crypto_provider();
    let ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    );
    Ok((ca, config, vetted("127.0.0.1:9".parse()?)))
}

#[tokio::test(start_paused = true)]
async fn a_client_that_never_speaks_is_cut_off_at_the_handshake_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let (ca, config, upstream) = handshake_probe()?;
    let (_peer, client) = tokio::io::duplex(1024);
    let outcome = super::terminate_and_forward(
        PrefixedStream::new(client, vec![]),
        "app.example.com",
        &ca,
        upstream,
        UpstreamMarking::NotNeeded,
        config,
        None,
        None,
    )
    .await;
    assert!(matches!(
        outcome,
        Err(TlsInterceptError::ClientHandshakeTimedOut { wait, .. })
            if wait == crate::tunnel_deadlines::HANDSHAKE_TIMEOUT
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_trickles_a_partial_hello_is_cut_off_at_the_handshake_deadline()
-> Result<(), Box<dyn std::error::Error>> {
    let (ca, config, upstream) = handshake_probe()?;
    let (mut peer, client) = tokio::io::duplex(1024);
    peer.write_all(&[0x16, 0x03, 0x01]).await?;
    let outcome = super::terminate_and_forward(
        PrefixedStream::new(client, vec![]),
        "app.example.com",
        &ca,
        upstream,
        UpstreamMarking::NotNeeded,
        config,
        None,
        None,
    )
    .await;
    assert!(matches!(
        outcome,
        Err(TlsInterceptError::ClientHandshakeTimedOut { .. })
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_handshake_timeout_releases_the_tunnel_slot() -> Result<(), Box<dyn std::error::Error>> {
    let (ca, config, upstream) = handshake_probe()?;
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = slots.clone().acquire_owned().await?;
    let (_peer, client) = tokio::io::duplex(1024);
    let tunnel = tokio::spawn(async move {
        let _slot = permit;
        super::terminate_and_forward(
            PrefixedStream::new(client, vec![]),
            "app.example.com",
            &ca,
            upstream,
            UpstreamMarking::NotNeeded,
            config,
            None,
            None,
        )
        .await
    });
    assert_eq!(slots.available_permits(), 0);
    assert!(tunnel.await?.is_err());
    assert_eq!(slots.available_permits(), 1);
    Ok(())
}

pub(crate) fn client_trusting(ca: &SandboxCa) -> Result<TlsConnector, TestError> {
    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(ca.root_cert_pem().into_bytes());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    Ok(TlsConnector::from(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    )))
}

#[tokio::test]
async fn a_client_that_stops_reading_is_cut_off_and_frees_its_slot() -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let hostname = "api.example.com";
    let upstream_ca = build_test_upstream_ca_and_leaf(hostname)?;
    let upstream_acceptor = TlsAcceptor::from(Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![upstream_ca.cert.clone()], upstream_ca.key.clone_key())?,
    ));
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;
    tokio::spawn(async move {
        let Ok((tcp, _)) = upstream_listener.accept().await else {
            return;
        };
        let Ok(mut tls) = upstream_acceptor.accept(tcp).await else {
            return;
        };
        let mut head = Vec::new();
        let mut chunk = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match tls.read(&mut chunk).await {
                Ok(n) if n > 0 => head.extend_from_slice(&chunk[..n]),
                _ => return,
            }
        }
        let _ = tls
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4294967296\r\n\r\n")
            .await;
        let block = vec![b'x'; 64 * 1024];
        while tls.write_all(&block).await.is_ok() {}
    });

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let connector = client_trusting(&sandbox_ca)?;
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&upstream_ca.pem)?)
            .with_no_client_auth(),
    );
    let deadlines = TunnelDeadlines {
        stalled: Duration::from_millis(300),
        max_queued_for_client: crate::tunnel_deadlines::ByteBudget::from_kib(16),
        ..patient_deadlines()
    };

    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = slots.clone().acquire_owned().await?;
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    let proxy_task = tokio::spawn(async move {
        let _slot = permit;
        let (client_tcp, _) = proxy_listener.accept().await?;
        let outcome = terminate_and_forward_with_secrets(
            PrefixedStream::new(client_tcp, vec![]),
            hostname,
            &sandbox_ca,
            vetted(upstream_addr),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            None,
            None,
            None,
            deadlines,
        )
        .await;
        Ok::<_, std::io::Error>(outcome)
    });

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;
    client_tls
        .write_all(
            format!(
                "GET /big HTTP/1.1\r\nHost: api.example.com:{}\r\n\r\n",
                upstream_addr.port()
            )
            .as_bytes(),
        )
        .await?;

    let outcome = timeout(Duration::from_secs(20), proxy_task).await???;
    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(slots.available_permits(), 1);
    drop(client_tls);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bytes_queued_for_a_client_that_never_reads_stay_within_the_limit() -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let hostname = "api.example.com";
    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let connector = client_trusting(&sandbox_ca)?;
    let acceptor = TlsAcceptor::from(server_config_for_leaf(sandbox_ca.mint_leaf_cert(hostname)?));

    let limit = crate::tunnel_deadlines::ByteBudget::from_kib(8);
    let transport = 4096;
    let (client_end, server_end) = tokio::io::duplex(transport);
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let (client, server) = tokio::join!(connector.connect(server_name, client_end), acceptor.accept(server_end));
    let _client = client?;
    let mut server = server?;
    server.get_mut().1.set_buffer_limit(Some(limit.bytes()));

    let block = [0u8; 1024];
    let mut accepted = 0usize;
    while let Ok(Ok(n)) = timeout(Duration::from_secs(1), server.write(&block)).await {
        accepted += n;
        assert!(accepted < 1024 * 1024, "writes were never held back");
    }
    assert!(accepted > 0);
    assert!(
        accepted <= limit.bytes() + transport + 16 * 1024,
        "{accepted} bytes accepted for a client that reads nothing"
    );
    Ok(())
}

struct AllowEveryRequest;

#[async_trait::async_trait]
impl crate::request_filter::RequestFilter for AllowEveryRequest {
    async fn decide(
        &self,
        _request: &crate::request_filter::RequestView<'_>,
    ) -> crate::request_filter::RequestDecision {
        crate::request_filter::RequestDecision::Allow
    }
}

#[derive(Debug)]
struct AcceptAnyCertificate;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::CryptoProvider::get_default()
            .map(|provider| provider.signature_verification_algorithms.supported_schemes())
            .unwrap_or_default()
    }
}

mod naming_tests;
