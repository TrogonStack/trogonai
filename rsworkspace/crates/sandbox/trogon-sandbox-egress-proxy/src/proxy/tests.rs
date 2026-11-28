use std::time::Duration;

use super::*;
use crate::held_writes::{HeldWrites, WAIT, expect_bytes};
use crate::resolve::{NameLookup, SsrfFloor};
use crate::tls_ca::{PermittedDomains, install_default_crypto_provider};
use rcgen::string::Ia5String;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use std::net::SocketAddr;
use tokio::io::AsyncWriteExt as _;
use tokio::io::duplex;
use tokio::net::TcpListener;
use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_contract::{
    AllowRange, CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, ForwardUrl, HeaderName,
    IpRange, PullSecretId, TlsExclusions,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type TaskError = io::Error;

fn range(text: &str) -> Result<IpRange, Box<dyn std::error::Error>> {
    Ok(IpRange::new(text)?)
}

fn allow_range(text: &str) -> Result<AllowRange, Box<dyn std::error::Error>> {
    Ok(AllowRange::new(text)?)
}

#[tokio::test]
async fn a_prefix_is_flushed_upstream_before_a_raw_splice_waits_on_either_side() -> TestResult {
    let (mut client, proxy_client) = duplex(64 * 1024);
    let (proxy_upstream, mut upstream) = duplex(64 * 1024);
    let splice = tokio::spawn(super::splice_with_prefix(
        proxy_client,
        HeldWrites::new(proxy_upstream),
        b"\x16\x03\x01hello",
    ));
    expect_bytes(&mut upstream, b"\x16\x03\x01hello", "the prefix").await;
    client.shutdown().await?;
    upstream.shutdown().await?;
    let outcome = tokio::time::timeout(WAIT, splice).await??;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }));
    Ok(())
}

fn secrets() -> Result<SecretSubstitution, Box<dyn std::error::Error>> {
    Ok(crate::secret_substitution::tests::fixture()?.substitution)
}

#[test]
fn a_pass_through_connection_to_a_binding_host_is_terminated() -> TestResult {
    let secrets = secrets()?;
    let dispatch = dispatch_with_secrets(
        dispatch_for_action(Some("api.example.com"), Some(&DomainRuleAction::PassThrough)),
        Some("api.example.com"),
        443,
        Some(&secrets),
    );

    assert_eq!(
        dispatch,
        ConnectionDispatch::TlsIntercept {
            sni_host: "api.example.com",
            domain_action: None,
        }
    );
    Ok(())
}

#[test]
fn a_host_no_binding_injects_at_stays_a_raw_splice() -> TestResult {
    let secrets = secrets()?;

    let dispatch = dispatch_with_secrets(
        ConnectionDispatch::RawSplice,
        Some("other.example.net"),
        443,
        Some(&secrets),
    );
    assert_eq!(dispatch, ConnectionDispatch::RawSplice);
    Ok(())
}

#[test]
fn without_sni_or_secrets_the_dispatch_is_unchanged() -> TestResult {
    let secrets = secrets()?;

    assert_eq!(
        dispatch_with_secrets(ConnectionDispatch::RawSplice, None, 443, Some(&secrets)),
        ConnectionDispatch::RawSplice
    );
    assert_eq!(
        dispatch_with_secrets(ConnectionDispatch::RawSplice, Some("api.example.com"), 443, None),
        ConnectionDispatch::RawSplice
    );
    Ok(())
}

async fn read_fixed_http_response(
    client_tls: &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin),
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
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

fn test_floor() -> SsrfFloor {
    SsrfFloor::system().with_loopback_exception()
}

struct StubLookup(Vec<IpAddr>);

#[async_trait::async_trait]
impl NameLookup for StubLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(self.0.clone())
    }
}

struct UnresolvableLookup;

#[async_trait::async_trait]
impl NameLookup for UnresolvableLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Err(io::ErrorKind::NotFound.into())
    }
}

fn resolver_answering(answers: &[&str]) -> Result<VettingResolver, Box<dyn std::error::Error>> {
    let answers = answers.iter().map(|a| a.parse()).collect::<Result<Vec<IpAddr>, _>>()?;
    Ok(VettingResolver::new(Arc::new(StubLookup(answers)), test_floor()))
}

fn test_resolver() -> VettingResolver {
    VettingResolver::new(
        Arc::new(StubLookup(vec![IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)])),
        test_floor(),
    )
}

fn test_oidc_identity_context() -> Result<OidcIdentityContext, Box<dyn std::error::Error>> {
    Ok(OidcIdentityContext {
        signer: OidcSigner::generate()?,
        issuer: "sandbox-egress-proxy-test".to_string(),
        subject: SandboxId::new("sandbox-test")?,
        tenant_id: TenantId::new("tenant-test")?,
        lease_id: LeaseId::new("lease-test".to_string()),
    })
}

fn test_tls_intercept_context() -> Result<TlsInterceptContext, Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    );
    Ok(TlsInterceptContext::new(
        ca,
        upstream_tls_config,
        Arc::new(crate::credential_source::tests::FakeCredentialSource::new()),
        test_oidc_identity_context()?,
    )
    .with_deadlines(crate::tls_intercept::tests::patient_deadlines()))
}

#[test]
fn intercept_context_marks_upstream_sockets_unless_told_otherwise() -> TestResult {
    let context = test_tls_intercept_context()?;
    assert_eq!(context.marking, UpstreamMarking::Required);
    assert_eq!(
        context.with_marking(UpstreamMarking::NotNeeded).marking,
        UpstreamMarking::NotNeeded
    );
    Ok(())
}

#[test]
fn excluded_hosts_are_never_terminated_for_secrets() -> TestResult {
    let secrets = crate::secret_substitution::tests::fixture()?.substitution;
    let covered = |context: &TlsInterceptContext, host: &str| {
        context
            .secrets_for(Some(host), 443)
            .is_some_and(|secrets| secrets.covers(host, 443))
    };
    let plain = test_tls_intercept_context()?.with_secrets(secrets.clone());
    assert!(covered(&plain, "api.example.com"));

    let excluded = test_tls_intercept_context()?
        .with_secrets(secrets)
        .with_network_options(Arc::new(NetworkOptions {
            tls_exclusions: TlsExclusions::new(["api.example.com".parse()?]),
            ..NetworkOptions::default()
        }));
    assert!(!covered(&excluded, "api.example.com"));
    assert!(excluded.secrets_for(None, 443).is_some());
    Ok(())
}

#[test]
fn the_terminate_all_switch_is_off_unless_asked_for() -> TestResult {
    assert!(!test_tls_intercept_context()?.terminates_every_allowed_host());
    let terminating =
        test_tls_intercept_context()?.with_network_options(Arc::new(NetworkOptions::default().terminating()));
    assert!(terminating.terminates_every_allowed_host());
    Ok(())
}

fn test_credential_transform(
    match_rules: Vec<trogon_sandbox_contract::MatchRule>,
) -> Result<CredentialTransform, Box<dyn std::error::Error>> {
    Ok(CredentialTransform {
        match_rules,
        credential: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new("test-secret"),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
        injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
    })
}

#[tokio::test]
async fn allowed_plain_tcp_connection_is_spliced_byte_for_byte() -> TestResult {
    // Upstream "real destination" echo server.
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;

    let upstream_task = tokio::spawn(async move {
        let (mut sock, _) = upstream_listener.accept().await?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = sock.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            sock.write_all(&chunk[..n]).await?;
        }
        Ok::<_, TaskError>(buf)
    });

    // Proxy front door the "sandboxed process" connects to.
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let policy = NetworkPolicy::AllowAll;
    let tls_intercept = test_tls_intercept_context()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                upstream_addr.ip(),
                upstream_addr.port(),
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    let payload = b"hello upstream, this is plain tcp, not tls at all";
    client.write_all(payload).await?;

    let mut echoed = vec![0u8; payload.len()];
    client.read_exact(&mut echoed).await?;
    assert_eq!(&echoed, payload);

    // Close the client side so both the upstream echo loop and the
    // proxy's bidirectional copy see EOF and finish.
    drop(client);

    let outcome = proxy_task.await??;
    match outcome {
        ProxyOutcome::Completed { .. } => {}
        other => return Err(format!("expected Completed outcome, got {other:?}").into()),
    }

    let received_by_upstream = upstream_task.await??;
    assert_eq!(received_by_upstream, payload);
    Ok(())
}

#[tokio::test]
async fn denied_connection_never_reaches_upstream() -> TestResult {
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let policy = NetworkPolicy::DenyAll;
    let tls_intercept = test_tls_intercept_context()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        // Destination is deliberately a non-listening port; if the deny
        // path is broken and attempts to connect upstream, this will
        // surface as a connect failure instead of a clean Denied.
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                1,
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    let _ = client.write_all(b"anything").await;

    let outcome = proxy_task.await??;
    assert!(matches!(outcome, ProxyOutcome::Denied(_)));
    Ok(())
}

async fn deny_by_floor(dest: &str, floor: SsrfFloor) -> Result<ProxyOutcome, Box<dyn std::error::Error>> {
    let resolver = VettingResolver::new(Arc::new(StubLookup(Vec::new())), floor);
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    let tls_intercept = test_tls_intercept_context()?;
    let dest: IpAddr = dest.parse()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(handle_connection(client, &NetworkPolicy::AllowAll, dest, 80, &resolver, &tls_intercept).await)
    });
    let mut client = TcpStream::connect(proxy_addr).await?;
    let _ = client.write_all(b"GET / HTTP/1.1\r\n\r\n").await;
    Ok(proxy_task.await??)
}

#[tokio::test]
async fn transparent_path_refuses_a_metadata_original_destination() -> TestResult {
    match deny_by_floor("169.254.169.254", SsrfFloor::system()).await? {
        ProxyOutcome::Denied(denial) => assert_eq!(
            denial.reason,
            EgressViolationReason::DestinationAddressDenied(AddressDenialClass::LinkLocal)
        ),
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn transparent_path_refuses_a_loopback_original_destination_under_allow_all() -> TestResult {
    match deny_by_floor("127.0.0.1", SsrfFloor::system()).await? {
        ProxyOutcome::Denied(denial) => assert_eq!(
            denial.reason,
            EgressViolationReason::DestinationAddressDenied(AddressDenialClass::Loopback)
        ),
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn transparent_path_normalises_a_mapped_original_destination() -> TestResult {
    match deny_by_floor("::ffff:169.254.169.254", SsrfFloor::system()).await? {
        ProxyOutcome::Denied(denial) => assert_eq!(
            denial.reason,
            EgressViolationReason::DestinationAddressDenied(AddressDenialClass::LinkLocal)
        ),
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

trait OptionsExt: Sized {
    fn terminating(self) -> Self;
    fn requiring_termination(self) -> Self;
    fn strict(self) -> Self;
    fn exempting(self, exemptions: trogon_sandbox_contract::TlsInterceptionExemptions) -> Self;
}

impl OptionsExt for NetworkOptions {
    fn terminating(mut self) -> Self {
        self.terminate_allowed_tls = trogon_sandbox_contract::AllowedTlsTermination::Enabled;
        self
    }

    fn requiring_termination(mut self) -> Self {
        self.domain_only_allows = trogon_sandbox_contract::DomainOnlyAllows::RequireTermination;
        self
    }

    fn strict(mut self) -> Self {
        self.unlisted = trogon_sandbox_contract::UnlistedDestinations::Refuse;
        self
    }

    fn exempting(mut self, exemptions: trogon_sandbox_contract::TlsInterceptionExemptions) -> Self {
        self.tls_interception_exemptions = exemptions;
        self
    }
}

const SSL_REQUEST_LEN: u32 = 8;
const SSL_REQUEST_CODE: u32 = 80_877_103;

fn ssl_request_bytes() -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    buf.extend_from_slice(&SSL_REQUEST_LEN.to_be_bytes());
    buf.extend_from_slice(&SSL_REQUEST_CODE.to_be_bytes());
    buf
}

fn u16_be(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}

/// Builds a minimal well-formed TLS ClientHello record carrying a single
/// SNI host name extension, mirroring `sni::tests::build_client_hello_with_sni`.
fn client_hello_with_sni(hostname: &str) -> Vec<u8> {
    const CONTENT_TYPE_HANDSHAKE: u8 = 0x16;
    const HANDSHAKE_TYPE_CLIENT_HELLO: u8 = 0x01;
    const EXTENSION_TYPE_SERVER_NAME: u16 = 0x0000;
    const SERVER_NAME_TYPE_HOST_NAME: u8 = 0x00;

    let mut server_name_list = Vec::new();
    server_name_list.push(SERVER_NAME_TYPE_HOST_NAME);
    server_name_list.extend_from_slice(&u16_be(hostname.len() as u16));
    server_name_list.extend_from_slice(hostname.as_bytes());

    let mut sni_extension_data = Vec::new();
    sni_extension_data.extend_from_slice(&u16_be(server_name_list.len() as u16));
    sni_extension_data.extend_from_slice(&server_name_list);

    let mut extensions = Vec::new();
    extensions.extend_from_slice(&u16_be(EXTENSION_TYPE_SERVER_NAME));
    extensions.extend_from_slice(&u16_be(sni_extension_data.len() as u16));
    extensions.extend_from_slice(&sni_extension_data);

    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0u8; 32]);
    body.push(0);
    body.extend_from_slice(&u16_be(2));
    body.extend_from_slice(&[0x13, 0x01]);
    body.push(1);
    body.push(0);
    body.extend_from_slice(&u16_be(extensions.len() as u16));
    body.extend_from_slice(&extensions);

    let mut handshake = Vec::new();
    handshake.push(HANDSHAKE_TYPE_CLIENT_HELLO);
    let body_len = (body.len() as u32).to_be_bytes();
    handshake.extend_from_slice(&body_len[1..4]);
    handshake.extend_from_slice(&body);

    let mut record = Vec::new();
    record.push(CONTENT_TYPE_HANDSHAKE);
    record.extend_from_slice(&[0x03, 0x01]);
    record.extend_from_slice(&u16_be(handshake.len() as u16));
    record.extend_from_slice(&handshake);
    record
}

/// Spawns a fake Postgres upstream: reads the 8-byte `SSLRequest`, replies
/// with `reply_byte` (`S`/`N`/etc.), and if `reply_byte == b'S'` also reads
/// whatever ClientHello bytes the negotiation relays before EOF. Returns
/// everything received after the initial `SSLRequest`, so tests can assert
/// that a Deny decision lets nothing past negotiation reach upstream.
fn spawn_fake_pg_upstream(
    listener: TcpListener,
    reply_byte: u8,
) -> tokio::task::JoinHandle<Result<Vec<u8>, TaskError>> {
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await?;

        let mut ssl_request = [0u8; 8];
        sock.read_exact(&mut ssl_request).await?;
        assert_eq!(ssl_request, ssl_request_bytes().as_slice());

        sock.write_all(&[reply_byte]).await?;

        let mut received_after_negotiation = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = sock.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            received_after_negotiation.extend_from_slice(&chunk[..n]);
        }
        Ok(received_after_negotiation)
    })
}

#[tokio::test]
async fn postgres_tls_upgrade_with_matching_sni_is_spliced() -> TestResult {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;

    let upstream_task = tokio::spawn(async move {
        let (mut sock, _) = upstream_listener.accept().await?;

        let mut ssl_request = [0u8; 8];
        sock.read_exact(&mut ssl_request).await?;
        sock.write_all(b"S").await?;

        let hello = client_hello_with_sni("api.example.com");
        let mut received_hello = vec![0u8; hello.len()];
        sock.read_exact(&mut received_hello).await?;
        assert_eq!(received_hello, hello);

        let payload = b"post-tls application data";
        let mut echoed = vec![0u8; payload.len()];
        sock.read_exact(&mut echoed).await?;
        assert_eq!(&echoed, payload);
        sock.write_all(payload).await?;
        Ok::<(), TaskError>(())
    });

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let policy = NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "*.example.com".parse()?,
            action: trogon_sandbox_contract::DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let tls_intercept = test_tls_intercept_context()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                upstream_addr.ip(),
                upstream_addr.port(),
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    client.write_all(&ssl_request_bytes()).await?;

    let mut ssl_reply = [0u8; 1];
    client.read_exact(&mut ssl_reply).await?;
    assert_eq!(ssl_reply[0], b'S');

    let hello = client_hello_with_sni("api.example.com");
    client.write_all(&hello).await?;

    let payload = b"post-tls application data";
    client.write_all(payload).await?;

    let mut echoed = vec![0u8; payload.len()];
    client.read_exact(&mut echoed).await?;
    assert_eq!(&echoed, payload);

    drop(client);

    let outcome = proxy_task.await??;
    match outcome {
        ProxyOutcome::Completed { .. } => {}
        other => return Err(format!("expected Completed outcome, got {other:?}").into()),
    }

    upstream_task.await??;
    Ok(())
}

#[tokio::test]
async fn postgres_tls_upgrade_with_non_matching_sni_is_denied_and_upstream_sees_nothing_more() -> TestResult {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;
    let upstream_task = spawn_fake_pg_upstream(upstream_listener, b'S');

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let policy = NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "*.example.com".parse()?,
            action: trogon_sandbox_contract::DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let tls_intercept = test_tls_intercept_context()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                upstream_addr.ip(),
                upstream_addr.port(),
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    client.write_all(&ssl_request_bytes()).await?;

    let mut ssl_reply = [0u8; 1];
    client.read_exact(&mut ssl_reply).await?;
    assert_eq!(ssl_reply[0], b'S');

    let hello = client_hello_with_sni("evil.example.net");
    client.write_all(&hello).await?;

    // Anything written after this point must never reach upstream once
    // Deny is decided; the proxy must close both sockets first.
    let _ = client.write_all(b"should never arrive upstream").await;
    drop(client);

    let outcome = proxy_task.await??;
    assert!(matches!(outcome, ProxyOutcome::Denied(_)));

    let received_after_negotiation = upstream_task.await??;
    assert_eq!(received_after_negotiation, hello);
    Ok(())
}

#[tokio::test]
async fn postgres_plaintext_declined_is_always_denied() -> TestResult {
    // sslmode=require+ parity: a declined TLS upgrade must be denied even
    // when an IP-range rule would otherwise have allowed the destination -
    // otherwise a client could bypass all domain rules just by declining
    // TLS.
    for allow_ranges in [vec![], vec![allow_range("127.0.0.0/8")?]] {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
        let upstream_addr = upstream_listener.local_addr()?;

        let upstream_task = tokio::spawn(async move {
            let (mut sock, _) = upstream_listener.accept().await?;
            let mut ssl_request = [0u8; 8];
            sock.read_exact(&mut ssl_request).await?;
            sock.write_all(b"N").await?;
            // Nothing further should ever be written after a deny.
            let n = sock.read(&mut [0u8; 64]).await.unwrap_or(0);
            assert_eq!(n, 0, "upstream must see nothing past the N reply");
            Ok::<(), TaskError>(())
        });

        let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
        let proxy_addr = proxy_listener.local_addr()?;

        let policy = NetworkPolicy::UserDefined {
            rules: vec![trogon_sandbox_contract::DomainRule {
                domain: "*.example.com".parse()?,
                action: trogon_sandbox_contract::DomainRuleAction::PassThrough,
            }],
            allow_ranges,
            deny_ranges: vec![],
            denied_domains: vec![],
            opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
            request_rules: vec![],
        };
        let tls_intercept = test_tls_intercept_context()?;
        let proxy_task = tokio::spawn(async move {
            let (client, _) = proxy_listener.accept().await?;
            io::Result::Ok(
                handle_connection(
                    client,
                    &policy,
                    upstream_addr.ip(),
                    upstream_addr.port(),
                    &test_resolver(),
                    &tls_intercept,
                )
                .await,
            )
        });

        let mut client = TcpStream::connect(proxy_addr).await?;
        client.write_all(&ssl_request_bytes()).await?;

        let mut ssl_reply = [0u8; 1];
        client.read_exact(&mut ssl_reply).await?;
        assert_eq!(ssl_reply[0], b'N');

        drop(client);
        let outcome = proxy_task.await??;
        assert!(matches!(outcome, ProxyOutcome::Denied(_)));
        upstream_task.await??;
    }
    Ok(())
}

#[tokio::test]
async fn postgres_gss_enc_request_is_denied_without_contacting_upstream() -> TestResult {
    // sslmode=require+ parity: GSSAPI-encrypted negotiation is rejected
    // outright, before any upstream connection is attempted, since there
    // is no SNI-equivalent to evaluate domain rules against.
    let mut gss_request = Vec::with_capacity(8);
    gss_request.extend_from_slice(&8u32.to_be_bytes());
    gss_request.extend_from_slice(&80_877_104u32.to_be_bytes());

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    // No listener bound on this address/port: if the proxy tried to
    // connect upstream, it would fail with a connect error rather than
    // returning Denied.
    let unreachable_addr: SocketAddr = "127.0.0.1:1".parse()?;

    let policy = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![allow_range("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let tls_intercept = test_tls_intercept_context()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                unreachable_addr.ip(),
                unreachable_addr.port(),
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    client.write_all(&gss_request).await?;
    drop(client);

    let outcome = proxy_task.await??;
    assert!(matches!(outcome, ProxyOutcome::Denied(_)));
    Ok(())
}

#[tokio::test]
async fn pass_through_matched_sni_connection_is_spliced_byte_for_byte() -> TestResult {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;

    let upstream_task = tokio::spawn(async move {
        let (mut sock, _) = upstream_listener.accept().await?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = sock.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            sock.write_all(&chunk[..n]).await?;
        }
        Ok::<_, TaskError>(buf)
    });

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let policy = NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "*.example.com".parse()?,
            action: trogon_sandbox_contract::DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let tls_intercept = test_tls_intercept_context()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                upstream_addr.ip(),
                upstream_addr.port(),
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    let hello = client_hello_with_sni("api.example.com");
    client.write_all(&hello).await?;

    let payload = b"raw splice, no tls termination";
    client.write_all(payload).await?;

    // The upstream is a raw echo loop with no message framing, so it
    // echoes the ClientHello bytes back before the payload bytes: read
    // the whole echo and check the tail, rather than assuming the first
    // `payload.len()` bytes read back are already the payload's echo.
    let mut echoed = vec![0u8; hello.len() + payload.len()];
    client.read_exact(&mut echoed).await?;
    assert_eq!(&echoed[..hello.len()], hello.as_slice());
    assert_eq!(&echoed[hello.len()..], payload);

    drop(client);

    let outcome = proxy_task.await??;
    match outcome {
        ProxyOutcome::Completed { .. } => {}
        other => return Err(format!("expected Completed outcome, got {other:?}").into()),
    }

    let received = upstream_task.await??;
    assert_eq!(&received[..hello.len()], hello.as_slice());
    assert_eq!(&received[hello.len()..], payload);
    Ok(())
}

fn rule_policy(allow_ranges: &[&str]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "*.example.com".parse()?,
            action: trogon_sandbox_contract::DomainRuleAction::PassThrough,
        }],
        allow_ranges: allow_ranges
            .iter()
            .map(|r| allow_range(r))
            .collect::<Result<Vec<_>, _>>()?,
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

/// Drives one transparent connection to a real loopback upstream and
/// reports the outcome and whether the upstream was ever dialed.
async fn transparent_connection(
    policy: NetworkPolicy,
    resolver: VettingResolver,
    first_bytes: Vec<u8>,
    dest_ip: fn(IpAddr) -> IpAddr,
) -> Result<(ProxyOutcome, bool), Box<dyn std::error::Error>> {
    transparent_connection_with(policy, resolver, first_bytes, dest_ip, None).await
}

async fn transparent_connection_with(
    policy: NetworkPolicy,
    resolver: VettingResolver,
    first_bytes: Vec<u8>,
    dest_ip: fn(IpAddr) -> IpAddr,
    decider: Option<Arc<Decider>>,
) -> Result<(ProxyOutcome, bool), Box<dyn std::error::Error>> {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut sock, _) = upstream_listener.accept().await?;
        let mut sink = Vec::new();
        let _ = sock.read_to_end(&mut sink).await;
        io::Result::Ok(())
    });
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    let mut tls_intercept = test_tls_intercept_context()?;
    if let Some(decider) = decider {
        tls_intercept = tls_intercept.with_decider(decider);
    }
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                dest_ip(upstream_addr.ip()),
                upstream_addr.port(),
                &resolver,
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    client.write_all(&first_bytes).await?;
    client.shutdown().await?;
    let outcome = proxy_task.await??;
    let dialed = timeout(Duration::from_millis(200), upstream_task).await.is_ok();
    Ok((outcome, dialed))
}

fn same_ip(ip: IpAddr) -> IpAddr {
    ip
}

fn mapped_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V6(v4.to_ipv6_mapped()),
        IpAddr::V6(_) => ip,
    }
}

fn denied_reason(outcome: ProxyOutcome) -> Result<EgressViolationReason, Box<dyn std::error::Error>> {
    match outcome {
        ProxyOutcome::Denied(denial) => Ok(denial.reason),
        other => Err(format!("expected Denied outcome, got {other:?}").into()),
    }
}

struct AllowingTransport {
    asked: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::decider::DeciderTransport for AllowingTransport {
    async fn request(
        &self,
        _subject: String,
        _payload: Vec<u8>,
    ) -> Result<Vec<u8>, crate::decider::DeciderTransportError> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(br#"{"verdict":"allow","scope":"once"}"#.to_vec())
    }
}

struct NoAmend;

#[async_trait::async_trait]
impl crate::decider::PolicyAmender for NoAmend {
    async fn amend(&self, _host: &Host, _port: Port, _decision: Decision) {}
}

fn allowing_decider() -> (Arc<Decider>, Arc<AllowingTransport>) {
    let transport = Arc::new(AllowingTransport {
        asked: std::sync::atomic::AtomicUsize::new(0),
    });
    let decider = Arc::new(Decider::new(
        LeaseId::new("lease-test".to_string()),
        transport.clone(),
        Arc::new(NoAmend),
        crate::decider::DeciderLimits::default(),
    ));
    (decider, transport)
}

fn unlisted_policy() -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

#[tokio::test]
async fn a_decider_allowed_name_over_an_address_it_does_not_resolve_to_is_denied_undialed() -> TestResult {
    let (decider, transport) = allowing_decider();
    let (outcome, dialed) = transparent_connection_with(
        unlisted_policy(),
        resolver_answering(&["203.0.113.9"])?,
        client_hello_with_sni("api.example.com"),
        same_ip,
        Some(decider),
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::SniDestinationMismatch);
    assert!(!dialed);
    assert_eq!(transport.asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_decider_allowed_name_that_does_not_resolve_is_denied_undialed() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(UnresolvableLookup), test_floor());
    let (decider, _) = allowing_decider();
    let (outcome, dialed) = transparent_connection_with(
        unlisted_policy(),
        resolver,
        client_hello_with_sni("api.example.com"),
        same_ip,
        Some(decider),
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::SniResolutionFailed);
    assert!(!dialed);
    Ok(())
}

#[tokio::test]
async fn a_decider_allowed_name_over_one_of_its_addresses_is_dialed() -> TestResult {
    let (decider, _) = allowing_decider();
    let (outcome, dialed) = transparent_connection_with(
        unlisted_policy(),
        resolver_answering(&["203.0.113.9", "127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        same_ip,
        Some(decider),
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }), "{outcome:?}");
    assert!(dialed);
    Ok(())
}

#[tokio::test]
async fn a_decider_allow_does_not_lift_the_floor_and_is_not_even_asked() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(StubLookup(vec!["127.0.0.1".parse()?])), SsrfFloor::system());
    let (decider, transport) = allowing_decider();
    let (outcome, dialed) = transparent_connection_with(
        unlisted_policy(),
        resolver,
        client_hello_with_sni("api.example.com"),
        same_ip,
        Some(decider),
    )
    .await?;
    assert_eq!(
        denied_reason(outcome)?,
        EgressViolationReason::DestinationAddressDenied(AddressDenialClass::Loopback)
    );
    assert!(!dialed);
    assert_eq!(transport.asked.load(std::sync::atomic::Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn an_allowed_name_over_an_address_it_does_not_resolve_to_is_denied_undialed() -> TestResult {
    let (outcome, dialed) = transparent_connection(
        rule_policy(&[])?,
        resolver_answering(&["203.0.113.9"])?,
        client_hello_with_sni("api.example.com"),
        same_ip,
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::SniDestinationMismatch);
    assert!(!dialed);
    Ok(())
}

#[tokio::test]
async fn an_allowed_name_that_does_not_resolve_is_denied_undialed() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(UnresolvableLookup), test_floor());
    let (outcome, dialed) = transparent_connection(
        rule_policy(&[])?,
        resolver,
        client_hello_with_sni("api.example.com"),
        same_ip,
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::SniResolutionFailed);
    assert!(!dialed);
    Ok(())
}

#[tokio::test]
async fn an_allowed_name_over_one_of_its_addresses_is_dialed() -> TestResult {
    let (outcome, dialed) = transparent_connection(
        rule_policy(&[])?,
        resolver_answering(&["203.0.113.9", "127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        same_ip,
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }), "{outcome:?}");
    assert!(dialed);
    Ok(())
}

#[tokio::test]
async fn a_mapped_original_destination_matches_the_ipv4_answer() -> TestResult {
    let (outcome, dialed) = transparent_connection(
        rule_policy(&[])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        mapped_ip,
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }), "{outcome:?}");
    assert!(dialed);
    Ok(())
}

#[tokio::test]
async fn the_floor_still_applies_when_the_name_resolves_to_the_destination() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(StubLookup(vec!["127.0.0.1".parse()?])), SsrfFloor::system());
    let (outcome, dialed) = transparent_connection(
        rule_policy(&[])?,
        resolver,
        client_hello_with_sni("api.example.com"),
        same_ip,
    )
    .await?;
    assert_eq!(
        denied_reason(outcome)?,
        EgressViolationReason::DestinationAddressDenied(AddressDenialClass::Loopback)
    );
    assert!(!dialed);
    Ok(())
}

#[tokio::test]
async fn a_connection_without_sni_is_not_checked_against_dns() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(UnresolvableLookup), test_floor());
    let (outcome, dialed) = transparent_connection(
        rule_policy(&["127.0.0.0/8"])?,
        resolver,
        b"plain tcp, no client hello".to_vec(),
        same_ip,
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }), "{outcome:?}");
    assert!(dialed);
    Ok(())
}

#[tokio::test]
async fn an_allow_by_address_range_is_not_checked_against_the_name() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(UnresolvableLookup), test_floor());
    let (outcome, dialed) = transparent_connection(
        rule_policy(&["127.0.0.0/8"])?,
        resolver,
        client_hello_with_sni("unrelated.test"),
        same_ip,
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }), "{outcome:?}");
    assert!(dialed);
    Ok(())
}

#[tokio::test]
async fn allow_all_does_not_check_the_name() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(UnresolvableLookup), test_floor());
    let (outcome, dialed) = transparent_connection(
        NetworkPolicy::AllowAll,
        resolver,
        client_hello_with_sni("api.example.com"),
        same_ip,
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }), "{outcome:?}");
    assert!(dialed);
    Ok(())
}

struct TestUpstreamCa {
    pem: String,
    cert: rustls::pki_types::CertificateDer<'static>,
    key: rustls::pki_types::PrivateKeyDer<'static>,
}

fn build_test_upstream_ca_and_leaf(hostname: &str) -> Result<TestUpstreamCa, Box<dyn std::error::Error>> {
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

fn root_store_from_pem(pem: &str) -> Result<rustls::RootCertStore, Box<dyn std::error::Error>> {
    let mut root_store = rustls::RootCertStore::empty();
    let mut reader = std::io::Cursor::new(pem.as_bytes().to_vec());
    for cert in rustls_pemfile::certs(&mut reader) {
        root_store.add(cert?)?;
    }
    Ok(root_store)
}

#[tokio::test]
async fn transform_matched_sni_connection_injects_credential_into_upstream_request() -> TestResult {
    install_default_crypto_provider();
    let hostname = "api.example.com";

    let upstream_ca = build_test_upstream_ca_and_leaf(hostname)?;
    let upstream_server_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![upstream_ca.cert.clone()], upstream_ca.key)?,
    );
    let upstream_acceptor = tokio_rustls::TlsAcceptor::from(upstream_server_config);

    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;

    let upstream_task = tokio::spawn(async move {
        let (tcp, _) = upstream_listener.accept().await?;
        let mut tls = upstream_acceptor.accept(tcp).await?;

        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = tls.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        let body = b"ok";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        tls.write_all(response.as_bytes()).await?;
        tls.write_all(body).await?;
        tls.shutdown().await?;

        Ok::<_, TaskError>(buf)
    });

    let ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let sandbox_root_pem = ca.root_cert_pem();
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&upstream_ca.pem)?)
            .with_no_client_auth(),
    );
    let credential_source = Arc::new(
        crate::credential_source::tests::FakeCredentialSource::new().with_token("test-secret", "s3cr3t-token"),
    );
    let tls_intercept = TlsInterceptContext::new(
        ca,
        upstream_tls_config,
        credential_source,
        test_oidc_identity_context()?,
    )
    .with_deadlines(crate::tls_intercept::tests::patient_deadlines());

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let policy = NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "*.example.com".parse()?,
            action: trogon_sandbox_contract::DomainRuleAction::Transform(test_credential_transform(vec![])?),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                upstream_addr.ip(),
                upstream_addr.port(),
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
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
    let connector = tokio_rustls::TlsConnector::from(client_config);

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = rustls::pki_types::ServerName::try_from(hostname)?;
    let mut client_tls = connector.connect(server_name, tcp).await?;

    tokio::io::AsyncWriteExt::write_all(
        &mut client_tls,
        format!(
            "GET /v2/repo/manifests/latest HTTP/1.1\r\nHost: api.example.com:{}\r\n\r\n",
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

    let outcome = proxy_task.await??;
    match outcome {
        ProxyOutcome::Completed { .. } => {}
        other => return Err(format!("expected Completed outcome, got {other:?}").into()),
    }

    let received_by_upstream = upstream_task.await??;
    let received_text = String::from_utf8(received_by_upstream)?;
    assert!(
        received_text
            .to_ascii_lowercase()
            .contains("authorization: s3cr3t-token"),
        "expected injected Authorization header in request upstream saw, got: {received_text}"
    );
    Ok(())
}

#[tokio::test]
async fn transform_matched_rule_with_no_sni_returns_defensive_outcome_without_panicking() -> TestResult {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;
    let upstream_task = tokio::spawn(async move { upstream_listener.accept().await });

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let policy = NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "*.example.com".parse()?,
            action: trogon_sandbox_contract::DomainRuleAction::Transform(test_credential_transform(vec![])?),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let tls_intercept = test_tls_intercept_context()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                upstream_addr.ip(),
                upstream_addr.port(),
                &test_resolver(),
                &tls_intercept,
            )
            .await,
        )
    });

    let mut client = TcpStream::connect(proxy_addr).await?;
    client.write_all(b"not a tls client hello, no sni possible").await?;
    drop(client);

    let outcome = proxy_task.await??;
    assert!(matches!(outcome, ProxyOutcome::Denied(_)));

    upstream_task.abort();
    Ok(())
}

#[test]
fn dispatch_for_action_returns_tls_intercept_requires_sni_when_sni_is_none() -> TestResult {
    let action = trogon_sandbox_contract::DomainRuleAction::Transform(test_credential_transform(vec![])?);
    assert!(matches!(
        dispatch_for_action(None, Some(&action)),
        ConnectionDispatch::TlsInterceptRequiresSni
    ));

    let forward_action = trogon_sandbox_contract::DomainRuleAction::Forward(trogon_sandbox_contract::RequestForward {
        forward_url: ForwardUrl::new("https://example.com")?,
        match_rules: vec![],
        ttl_seconds: trogon_sandbox_contract::RequestForward::DEFAULT_TTL_SECONDS,
    });
    assert!(matches!(
        dispatch_for_action(None, Some(&forward_action)),
        ConnectionDispatch::TlsInterceptRequiresSni
    ));
    Ok(())
}

#[test]
fn dispatch_for_action_takes_raw_splice_for_pass_through_and_none() -> TestResult {
    assert!(matches!(dispatch_for_action(None, None), ConnectionDispatch::RawSplice));
    assert!(matches!(
        dispatch_for_action(None, Some(&trogon_sandbox_contract::DomainRuleAction::PassThrough)),
        ConnectionDispatch::RawSplice
    ));
    Ok(())
}

#[test]
fn dispatch_for_action_takes_tls_intercept_when_sni_and_transform_present() -> TestResult {
    let transform = test_credential_transform(vec![])?;
    let action = trogon_sandbox_contract::DomainRuleAction::Transform(transform.clone());
    assert!(matches!(
        dispatch_for_action(Some("api.example.com"), Some(&action)),
        ConnectionDispatch::TlsIntercept {
            sni_host: "api.example.com",
            domain_action: Some(DomainAction::Transform(t)),
        } if *t == transform
    ));
    Ok(())
}

#[test]
fn dispatch_for_action_takes_tls_intercept_when_sni_and_forward_present() -> TestResult {
    let forward = trogon_sandbox_contract::RequestForward {
        forward_url: ForwardUrl::new("https://hooks.example.com/receive")?,
        match_rules: vec![],
        ttl_seconds: trogon_sandbox_contract::RequestForward::DEFAULT_TTL_SECONDS,
    };
    let action = trogon_sandbox_contract::DomainRuleAction::Forward(forward.clone());
    assert!(matches!(
        dispatch_for_action(Some("api.example.com"), Some(&action)),
        ConnectionDispatch::TlsIntercept {
            sni_host: "api.example.com",
            domain_action: Some(DomainAction::Forward(f)),
        } if *f == forward
    ));
    Ok(())
}

// ---- Binding attribution and interception exemptions (rows 7.1, 7.2) ----

async fn transparent_connection_configured(
    policy: NetworkPolicy,
    resolver: VettingResolver,
    first_bytes: Vec<u8>,
    options: NetworkOptions,
) -> Result<(ProxyOutcome, bool), Box<dyn std::error::Error>> {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut sock, _) = upstream_listener.accept().await?;
        let mut sink = Vec::new();
        let _ = sock.read_to_end(&mut sink).await;
        io::Result::Ok(())
    });
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    let tls_intercept = test_tls_intercept_context()?.with_network_options(Arc::new(options));
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(
            handle_connection(
                client,
                &policy,
                upstream_addr.ip(),
                upstream_addr.port(),
                &resolver,
                &tls_intercept,
            )
            .await,
        )
    });
    let mut client = TcpStream::connect(proxy_addr).await?;
    client.write_all(&first_bytes).await?;
    client.shutdown().await?;
    let outcome = proxy_task.await??;
    let dialed = timeout(Duration::from_millis(200), upstream_task).await.is_ok();
    Ok((outcome, dialed))
}

fn is_terminated_not_relayed(outcome: &ProxyOutcome) -> bool {
    matches!(
        outcome,
        ProxyOutcome::TlsInterceptFailed(_) | ProxyOutcome::TlsInterceptRequiresSni
    )
}

fn exempt_host(host: &str) -> Result<trogon_sandbox_contract::TlsInterceptionExemptions, Box<dyn std::error::Error>> {
    Ok(trogon_sandbox_contract::TlsInterceptionExemptions::new(
        [host.parse()?],
        [],
    ))
}

fn exempt_address(
    text: &str,
) -> Result<trogon_sandbox_contract::TlsInterceptionExemptions, Box<dyn std::error::Error>> {
    Ok(trogon_sandbox_contract::TlsInterceptionExemptions::new(
        [],
        [range(text)?],
    ))
}

fn assert_relayed(outcome: &ProxyOutcome, dialed: bool) {
    assert!(matches!(outcome, ProxyOutcome::Completed { .. }), "{outcome:?}");
    assert!(dialed);
}

// 7.1: a shared edge address.

#[tokio::test]
async fn an_allowed_and_a_disallowed_name_on_one_address_are_told_apart() -> TestResult {
    let (outcome, dialed) = transparent_connection(
        rule_policy(&[])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        same_ip,
    )
    .await?;
    assert_relayed(&outcome, dialed);

    // The same edge address answers for a name the policy does not allow.
    let (outcome, dialed) = transparent_connection(
        rule_policy(&[])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("tenant.unrelated.net"),
        same_ip,
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::PolicyDenied);
    assert!(!dialed);
    Ok(())
}

#[tokio::test]
async fn only_the_proxys_own_resolution_binds_a_name_to_an_address() -> TestResult {
    let resolver = resolver_answering(&["127.0.0.1"])?;
    let allowed = Host::new("api.example.com")?;
    let other = Host::new("tenant.unrelated.net")?;
    let edge: IpAddr = "127.0.0.1".parse()?;
    assert!(!resolver.bindings().is_bound(&allowed, edge));

    resolver
        .confirm_binding(&allowed, Port::new(443)?, edge)
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert!(resolver.bindings().is_bound(&allowed, edge));
    assert!(!resolver.bindings().is_bound(&other, edge));
    Ok(())
}

#[tokio::test]
async fn domain_only_allows_pass_through_by_default() -> TestResult {
    let (outcome, dialed) = transparent_connection_configured(
        rule_policy(&[])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default(),
    )
    .await?;
    assert_relayed(&outcome, dialed);
    Ok(())
}

#[tokio::test]
async fn a_domain_only_allow_is_terminated_when_the_operator_requires_it() -> TestResult {
    let (outcome, _) = transparent_connection_configured(
        rule_policy(&[])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default().requiring_termination(),
    )
    .await?;
    assert!(is_terminated_not_relayed(&outcome), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn an_exact_address_allow_is_not_subject_to_required_termination() -> TestResult {
    let (outcome, dialed) = transparent_connection_configured(
        rule_policy(&["127.0.0.1/32"])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default().requiring_termination(),
    )
    .await?;
    assert_relayed(&outcome, dialed);
    Ok(())
}

#[tokio::test]
async fn a_wider_allow_range_does_not_count_as_an_exact_address_allow() -> TestResult {
    let (outcome, _) = transparent_connection_configured(
        rule_policy(&["127.0.0.0/8"])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default().requiring_termination(),
    )
    .await?;
    assert!(is_terminated_not_relayed(&outcome), "{outcome:?}");
    Ok(())
}

// 7.2: exemptions.

#[tokio::test]
async fn without_an_exemption_terminate_all_terminates_an_allowed_host() -> TestResult {
    let (outcome, _) = transparent_connection_configured(
        rule_policy(&[])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default().terminating(),
    )
    .await?;
    assert!(is_terminated_not_relayed(&outcome), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_hostname_exemption_with_a_live_binding_relays_without_termination() -> TestResult {
    let (outcome, dialed) = transparent_connection_configured(
        rule_policy(&[])?,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default()
            .terminating()
            .exempting(exempt_host("api.example.com")?),
    )
    .await?;
    assert_relayed(&outcome, dialed);
    Ok(())
}

#[tokio::test]
async fn a_hostname_exemption_without_a_binding_does_not_apply() -> TestResult {
    // Admitted by address, so the allow itself demanded no name binding;
    // the exemption alone must not buy an uninspected tunnel.
    let (outcome, _) = transparent_connection_configured(
        rule_policy(&["127.0.0.1/32"])?,
        resolver_answering(&["203.0.113.9"])?,
        client_hello_with_sni("other.exempt.test"),
        NetworkOptions::default()
            .terminating()
            .exempting(exempt_host("other.exempt.test")?),
    )
    .await?;
    assert!(is_terminated_not_relayed(&outcome), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn an_exemption_grants_no_egress() -> TestResult {
    for policy in [NetworkPolicy::DenyAll, unlisted_policy()] {
        let (outcome, dialed) = transparent_connection_configured(
            policy,
            resolver_answering(&["127.0.0.1"])?,
            client_hello_with_sni("api.example.com"),
            NetworkOptions::default()
                .strict()
                .exempting(trogon_sandbox_contract::TlsInterceptionExemptions::new(
                    ["api.example.com".parse()?],
                    [range("127.0.0.0/8")?],
                )),
        )
        .await?;
        assert_eq!(denied_reason(outcome)?, EgressViolationReason::PolicyDenied);
        assert!(!dialed);
    }
    Ok(())
}

#[tokio::test]
async fn an_exemption_does_not_lift_the_address_floor() -> TestResult {
    let resolver = VettingResolver::new(Arc::new(StubLookup(vec!["127.0.0.1".parse()?])), SsrfFloor::system());
    let (outcome, dialed) = transparent_connection_configured(
        NetworkPolicy::AllowAll,
        resolver,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default().exempting(exempt_address("127.0.0.0/8")?),
    )
    .await?;
    assert_eq!(
        denied_reason(outcome)?,
        EgressViolationReason::DestinationAddressDenied(AddressDenialClass::Loopback)
    );
    assert!(!dialed);
    Ok(())
}

#[tokio::test]
async fn an_exemption_does_not_admit_a_name_the_proxy_cannot_bind() -> TestResult {
    let (outcome, dialed) = transparent_connection_configured(
        rule_policy(&[])?,
        resolver_answering(&["203.0.113.9"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default().exempting(exempt_host("api.example.com")?),
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::SniDestinationMismatch);
    assert!(!dialed);
    Ok(())
}

#[tokio::test]
async fn an_exempt_flow_bypasses_credential_injection_and_inspection() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: "api.example.com".parse()?,
            action: trogon_sandbox_contract::DomainRuleAction::Transform(test_credential_transform(vec![])?),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let (terminated, _) = transparent_connection_configured(
        policy.clone(),
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default(),
    )
    .await?;
    assert!(is_terminated_not_relayed(&terminated), "{terminated:?}");

    let (outcome, dialed) = transparent_connection_configured(
        policy,
        resolver_answering(&["127.0.0.1"])?,
        client_hello_with_sni("api.example.com"),
        NetworkOptions::default().exempting(exempt_host("api.example.com")?),
    )
    .await?;
    assert_relayed(&outcome, dialed);
    Ok(())
}

#[tokio::test]
async fn an_address_exemption_covers_a_flow_with_no_name() -> TestResult {
    let context = test_tls_intercept_context()?.with_network_options(Arc::new(
        NetworkOptions::default().exempting(exempt_address("203.0.113.0/24")?),
    ));
    let resolver = resolver_answering(&[])?;
    for (ip, expected) in [
        ("203.0.113.7", true),
        ("::ffff:203.0.113.7", true),
        ("198.51.100.7", false),
    ] {
        let covered = exempt_from_interception(&context, &resolver, None, ip.parse()?, 443).await;
        assert_eq!(covered, expected, "{ip}");
    }
    Ok(())
}

#[tokio::test]
async fn a_hostname_exemption_never_covers_a_flow_with_no_name() -> TestResult {
    let context = test_tls_intercept_context()?.with_network_options(Arc::new(
        NetworkOptions::default().exempting(exempt_host("*.example.com")?),
    ));
    let resolver = resolver_answering(&["203.0.113.7"])?;
    let edge: IpAddr = "203.0.113.7".parse()?;
    for (sni, expected) in [
        (None, false),
        (Some("api.example.com"), true),
        (Some("api.example.net"), false),
    ] {
        let covered = exempt_from_interception(&context, &resolver, sni, edge, 443).await;
        assert_eq!(covered, expected, "{sni:?}");
    }
    Ok(())
}

// ---- Builders, sniffing, relaying and error paths ----

#[test]
fn context_builders_record_their_settings() -> TestResult {
    let mutator: Arc<dyn HeaderMutator> = Arc::new(|_: &mut Vec<(String, String)>, _: &Host| {});
    let context = test_tls_intercept_context()?
        .with_live_flows(LiveFlows::new())
        .with_header_mutator(mutator)
        .with_shared_queue(SharedQueueBudget::default())
        .with_deadlines(TunnelDeadlines::default())
        .with_network_options(Arc::new(NetworkOptions {
            handshake_timeout: Some(Duration::from_secs(3)),
            ..NetworkOptions::default()
        }));
    assert!(context.live_flows.is_some());
    assert!(context.header_mutator.is_some());
    assert_eq!(context.tunnel_deadlines().handshake, Duration::from_secs(3));
    Ok(())
}

struct Faulty {
    fail_reads: bool,
    fail_writes: bool,
}

impl AsyncRead for Faulty {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(match self.fail_reads {
            true => Err(io::ErrorKind::ConnectionReset.into()),
            false => Ok(()),
        })
    }
}

impl AsyncWrite for Faulty {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::task::Poll::Ready(match self.fail_writes {
            true => Err(io::ErrorKind::BrokenPipe.into()),
            false => Ok(buf.len()),
        })
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_prefix_that_cannot_be_written_upstream_fails_the_splice() {
    let (_peer, client) = duplex(64);
    let upstream = Faulty {
        fail_reads: false,
        fail_writes: true,
    };
    let outcome = splice_with_prefix(client, upstream, b"prefix").await;
    assert!(matches!(outcome, ProxyOutcome::UpstreamConnectFailed(_)));
}

#[tokio::test]
async fn a_relay_that_breaks_midway_reports_an_upstream_failure() {
    let (_peer, upstream) = duplex(64);
    let client = Faulty {
        fail_reads: true,
        fail_writes: false,
    };
    let outcome = splice_with_prefix(client, upstream, b"prefix").await;
    assert!(matches!(outcome, ProxyOutcome::UpstreamConnectFailed(_)));
}

async fn socket_pair() -> Result<(TcpStream, TcpStream), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let client = TcpStream::connect(listener.local_addr()?).await?;
    let (server, _) = listener.accept().await?;
    Ok((client, server))
}

async fn sniff_either(seeded: bool, stream: &mut TcpStream) -> io::Result<(Option<String>, Vec<u8>)> {
    if seeded {
        sniff_client_hello_with_seed(stream, Vec::new()).await
    } else {
        sniff_client_hello(stream).await
    }
}

#[tokio::test]
async fn a_hello_that_arrives_in_pieces_is_read_to_its_name() -> TestResult {
    for seeded in [false, true] {
        let (mut client, mut server) = socket_pair().await?;
        let hello = client_hello_with_sni("late.example.com");
        let (first, rest) = hello.split_at(10);
        let rest = rest.to_vec();
        client.write_all(first).await?;
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            client.write_all(&rest).await?;
            io::Result::Ok(client)
        });
        let (sni, bytes) = sniff_either(seeded, &mut server).await?;
        assert_eq!(sni.as_deref(), Some("late.example.com"));
        assert_eq!(bytes, hello);
        writer.await??;
    }
    Ok(())
}

#[tokio::test]
async fn a_client_that_closes_mid_hello_yields_no_name() -> TestResult {
    for seeded in [false, true] {
        let (mut client, mut server) = socket_pair().await?;
        let hello = client_hello_with_sni("late.example.com");
        client.write_all(&hello[..10]).await?;
        drop(client);
        let (sni, bytes) = sniff_either(seeded, &mut server).await?;
        assert_eq!(sni, None);
        assert_eq!(bytes, &hello[..10]);
    }
    Ok(())
}

#[tokio::test]
async fn a_client_that_stalls_mid_hello_yields_no_name_at_the_deadline() -> TestResult {
    for seeded in [false, true] {
        let (mut client, mut server) = socket_pair().await?;
        let hello = client_hello_with_sni("late.example.com");
        client.write_all(&hello[..10]).await?;
        let (sni, bytes) = sniff_either(seeded, &mut server).await?;
        assert_eq!(sni, None);
        assert_eq!(bytes, &hello[..10]);
        drop(client);
    }
    Ok(())
}

#[tokio::test]
async fn a_hello_that_never_completes_within_the_read_cap_yields_no_name() -> TestResult {
    for seeded in [false, true] {
        let (mut client, mut server) = socket_pair().await?;
        let mut filler = vec![0xABu8; 16 * 1024];
        filler[..5].copy_from_slice(&[0x16, 0x03, 0x01, 0xFF, 0xFF]);
        let writer = tokio::spawn(async move {
            client.write_all(&filler).await?;
            io::Result::Ok(client)
        });
        let (sni, bytes) = sniff_either(seeded, &mut server).await?;
        assert_eq!(sni, None);
        assert!(bytes.len() >= 16 * 1024);
        writer.await??;
    }
    Ok(())
}

#[tokio::test]
async fn a_seed_that_already_holds_the_hello_is_not_read_further() -> TestResult {
    let (_client, mut server) = socket_pair().await?;
    let hello = client_hello_with_sni("seeded.example.com");
    let (sni, bytes) = sniff_client_hello_with_seed(&mut server, hello.clone()).await?;
    assert_eq!(sni.as_deref(), Some("seeded.example.com"));
    assert_eq!(bytes, hello);
    Ok(())
}

async fn proxy_one(
    policy: NetworkPolicy,
    dest: SocketAddr,
    resolver: VettingResolver,
    context: TlsInterceptContext,
) -> Result<(TcpStream, tokio::task::JoinHandle<io::Result<ProxyOutcome>>), Box<dyn std::error::Error>> {
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    let proxy_task = tokio::spawn(async move {
        let (client, _) = proxy_listener.accept().await?;
        io::Result::Ok(handle_connection(client, &policy, dest.ip(), dest.port(), &resolver, &context).await)
    });
    Ok((TcpStream::connect(proxy_addr).await?, proxy_task))
}

fn loopback_ranges_policy() -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![allow_range("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

#[tokio::test]
async fn a_client_that_resets_before_sending_anything_fails_the_sniff() -> TestResult {
    let dest: SocketAddr = "127.0.0.1:1".parse()?;
    let (client, proxy_task) = proxy_one(
        loopback_ranges_policy()?,
        dest,
        test_resolver(),
        test_tls_intercept_context()?,
    )
    .await?;
    client.set_zero_linger()?;
    drop(client);
    assert!(matches!(proxy_task.await??, ProxyOutcome::SniffFailed(_)));
    Ok(())
}

#[tokio::test]
async fn a_client_that_resets_after_a_partial_hello_fails_the_sniff() -> TestResult {
    let dest: SocketAddr = "127.0.0.1:1".parse()?;
    let (mut client, proxy_task) = proxy_one(
        loopback_ranges_policy()?,
        dest,
        test_resolver(),
        test_tls_intercept_context()?,
    )
    .await?;
    client
        .write_all(&[0x16, 0x03, 0x01, 0xFF, 0xFF, 0x01, 0x00, 0x00])
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    client.set_zero_linger()?;
    drop(client);
    assert!(matches!(proxy_task.await??, ProxyOutcome::SniffFailed(_)));
    Ok(())
}

#[tokio::test]
async fn a_postgres_probe_is_refused_when_opaque_tunnels_are_refused() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![allow_range("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::Refuse,
        request_rules: vec![],
    };
    let dest: SocketAddr = "127.0.0.1:1".parse()?;
    let (mut client, proxy_task) = proxy_one(policy, dest, test_resolver(), test_tls_intercept_context()?).await?;
    client.write_all(&ssl_request_bytes()).await?;
    let reason = denied_reason(proxy_task.await??)?;
    assert_eq!(reason, EgressViolationReason::OpaqueTunnelRefused);
    Ok(())
}

#[tokio::test]
async fn a_postgres_probe_to_a_floored_address_is_denied() -> TestResult {
    let dest: SocketAddr = "169.254.169.254:5432".parse()?;
    let resolver = VettingResolver::new(Arc::new(StubLookup(Vec::new())), SsrfFloor::system());
    let (mut client, proxy_task) =
        proxy_one(NetworkPolicy::AllowAll, dest, resolver, test_tls_intercept_context()?).await?;
    client.write_all(&ssl_request_bytes()).await?;
    let reason = denied_reason(proxy_task.await??)?;
    assert_eq!(
        reason,
        EgressViolationReason::DestinationAddressDenied(AddressDenialClass::LinkLocal)
    );
    Ok(())
}

#[tokio::test]
async fn a_postgres_upstream_that_cannot_be_dialed_fails_the_connection() -> TestResult {
    let closed = TcpListener::bind("127.0.0.1:0").await?;
    let dest = closed.local_addr()?;
    drop(closed);
    let (mut client, proxy_task) = proxy_one(
        loopback_ranges_policy()?,
        dest,
        test_resolver(),
        test_tls_intercept_context()?.with_marking(UpstreamMarking::NotNeeded),
    )
    .await?;
    client.write_all(&ssl_request_bytes()).await?;
    assert!(matches!(
        proxy_task.await??,
        ProxyOutcome::PostgresUpstreamConnectFailed(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_postgres_upstream_that_hangs_up_fails_the_negotiation() -> TestResult {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let dest = upstream.local_addr()?;
    let hangup = tokio::spawn(async move {
        let (sock, _) = upstream.accept().await?;
        drop(sock);
        io::Result::Ok(())
    });
    let (mut client, proxy_task) = proxy_one(
        loopback_ranges_policy()?,
        dest,
        test_resolver(),
        test_tls_intercept_context()?.with_marking(UpstreamMarking::NotNeeded),
    )
    .await?;
    client.write_all(&ssl_request_bytes()).await?;
    assert!(matches!(proxy_task.await??, ProxyOutcome::PostgresNegotiationFailed(_)));
    hangup.await??;
    Ok(())
}

async fn postgres_upgrade_for(
    sni: &str,
    resolver: VettingResolver,
    options: NetworkOptions,
) -> Result<ProxyOutcome, Box<dyn std::error::Error>> {
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let dest = upstream_listener.local_addr()?;
    let upstream_task = spawn_fake_pg_upstream(upstream_listener, b'S');
    let context = test_tls_intercept_context()?
        .with_marking(UpstreamMarking::NotNeeded)
        .with_network_options(Arc::new(options));
    let (mut client, proxy_task) = proxy_one(rule_policy(&[])?, dest, resolver, context).await?;
    client.write_all(&ssl_request_bytes()).await?;
    let mut reply = [0u8; 1];
    client.read_exact(&mut reply).await?;
    client.write_all(&client_hello_with_sni(sni)).await?;
    let outcome = proxy_task.await??;
    drop(client);
    upstream_task.await??;
    Ok(outcome)
}

#[tokio::test]
async fn a_postgres_name_that_does_not_resolve_to_the_destination_is_denied() -> TestResult {
    let outcome = postgres_upgrade_for(
        "api.example.com",
        resolver_answering(&["203.0.113.9"])?,
        NetworkOptions::default(),
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::SniDestinationMismatch);
    Ok(())
}

#[tokio::test]
async fn a_postgres_name_only_allow_is_refused_when_termination_is_required() -> TestResult {
    let outcome = postgres_upgrade_for(
        "api.example.com",
        test_resolver(),
        NetworkOptions::default().requiring_termination(),
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::OpaqueTunnelRefused);
    Ok(())
}

#[tokio::test]
async fn a_flow_the_policy_no_longer_admits_is_cancelled_while_relayed() -> TestResult {
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let dest = upstream.local_addr()?;
    let upstream_task = tokio::spawn(async move {
        let (mut sock, _) = upstream.accept().await?;
        let mut sink = Vec::new();
        let _ = sock.read_to_end(&mut sink).await;
        io::Result::Ok(())
    });
    let flows = LiveFlows::new();
    let context = test_tls_intercept_context()?
        .with_marking(UpstreamMarking::NotNeeded)
        .with_live_flows(Arc::clone(&flows));
    let (mut client, proxy_task) = proxy_one(rule_policy(&[])?, dest, test_resolver(), context).await?;
    client.write_all(&client_hello_with_sni("api.example.com")).await?;
    for _ in 0..1000 {
        if !flows.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(flows.len(), 1);
    let cancelled = flows.sweep(&unlisted_policy(), &test_resolver()).await;
    assert_eq!(cancelled, 1);
    assert_eq!(denied_reason(proxy_task.await??)?, EgressViolationReason::PolicyDenied);
    drop(client);
    upstream_task.await??;
    Ok(())
}

#[tokio::test]
async fn a_name_that_is_not_a_host_cannot_be_bound() -> TestResult {
    let denial = confirm_sni_binding(&test_resolver(), true, Some("not a host"), "127.0.0.1".parse()?, 443)
        .await
        .ok_or("the name was accepted")?;
    assert_eq!(denial.reason, EgressViolationReason::SniResolutionFailed);
    Ok(())
}

async fn forwarded_with(
    sni: Option<&str>,
    action: Option<DomainRuleAction>,
    port: u16,
    opaque: OpaqueTunnelPolicy,
    prefix: Vec<u8>,
    interception: Interception,
) -> Result<ProxyOutcome, Box<dyn std::error::Error>> {
    let (_peer, client) = duplex(1024);
    let endpoint = crate::resolve::VettedAddr::assume_vetted(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)).with_port(port);
    let context = test_tls_intercept_context()?.with_marking(UpstreamMarking::NotNeeded);
    Ok(forward_decided(
        client,
        prefix,
        sni,
        action,
        endpoint.into(),
        &context,
        opaque,
        None,
        TunnelDeadlines {
            handshake: Duration::from_secs(300),
            first_request: Duration::from_secs(300),
            ..TunnelDeadlines::default()
        },
        interception,
    )
    .await)
}

#[tokio::test]
async fn a_raw_splice_to_an_undialable_upstream_fails() -> TestResult {
    let closed = TcpListener::bind("127.0.0.1:0").await?;
    let port = closed.local_addr()?.port();
    drop(closed);
    let outcome = forwarded_with(
        Some("example.com"),
        None,
        port,
        OpaqueTunnelPolicy::Allow,
        Vec::new(),
        Interception::Default,
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::UpstreamConnectFailed(_)));
    Ok(())
}

#[tokio::test]
async fn a_terminating_rule_without_a_name_is_reported() -> TestResult {
    let action = DomainRuleAction::Transform(test_credential_transform(vec![])?);
    let outcome = forwarded_with(
        None,
        Some(action),
        443,
        OpaqueTunnelPolicy::Allow,
        Vec::new(),
        Interception::Default,
    )
    .await?;
    assert!(matches!(outcome, ProxyOutcome::TlsInterceptRequiresSni));
    Ok(())
}

#[tokio::test]
async fn a_terminated_flow_that_does_not_start_like_tls_is_refused() -> TestResult {
    let outcome = forwarded_with(
        Some("example.com"),
        None,
        443,
        OpaqueTunnelPolicy::Refuse,
        b"GET / HTTP/1.1\r\n".to_vec(),
        Interception::Required,
    )
    .await?;
    assert_eq!(denied_reason(outcome)?, EgressViolationReason::OpaqueTunnelRefused);
    Ok(())
}
