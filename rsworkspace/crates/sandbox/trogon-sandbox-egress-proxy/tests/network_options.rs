use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use trogon_sandbox_contract::MitmUnixSocket;
use trogon_sandbox_contract::{
    AllowRange, DomainPattern, DomainRule, DomainRuleAction, EgressViolation, EgressViolationReason, LeaseId,
    MatchRule, NetworkPolicy, RequestForward, StringMatcher,
};
use trogon_sandbox_contract::{
    AllowedTlsTermination, DeniedDomainReasons, DenyReason, LoopbackNames, NetworkOptions, NoProxy, ParentProxy,
    ParentProxyUrl, TunnelCap, UpstreamCaCertificates,
};
use trogon_sandbox_contract::{DeniedDomain, ForwardUrl, Port};
use trogon_sandbox_contract::{ResponseHeaderStrip, StrippedHeader};
use trogon_sandbox_egress_proxy::TunnelGate;
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::resolve::{NameLookup, SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer, upstream_tls_config_trusting};
use trogon_sandbox_egress_proxy::tls_ca::{
    PermittedDomains, SandboxCa, install_default_crypto_provider, server_config_for_leaf,
};
use trogon_sandbox_egress_proxy::violation::{
    ViolationPublishError, ViolationRateLimit, ViolationReporter, ViolationSink,
};
use trogon_sandbox_egress_proxy::{
    CommandId, DenialReasons, IgnoreViolations, StoreViolationSink, ViolationFilter, ViolationStore,
};

const TOKEN: &str = "lease-token-value";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn domain_pattern(input: &str) -> TestResult<DomainPattern> {
    Ok(input.parse()?)
}

fn loopback_range() -> TestResult<AllowRange> {
    Ok(AllowRange::new("127.0.0.0/8")?)
}

fn terminating() -> NetworkOptions {
    NetworkOptions {
        terminate_allowed_tls: AllowedTlsTermination::from(true),
        ..NetworkOptions::default()
    }
}

fn with_tunnel_cap(mut options: NetworkOptions, cap: u32) -> TestResult<NetworkOptions> {
    options.tunnel_cap = Some(TunnelCap::new(cap)?);
    Ok(options)
}

#[derive(Default)]
struct InMemorySink(Mutex<Vec<Vec<u8>>>);

#[async_trait]
impl ViolationSink for InMemorySink {
    async fn publish(&self, _subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        locked(&self.0).push(payload);
        Ok(())
    }
}

impl InMemorySink {
    async fn wait_for(&self, count: usize) -> TestResult<Vec<EgressViolation>> {
        for _ in 0..100 {
            {
                let events = locked(&self.0);
                if events.len() >= count {
                    let parsed: Result<Vec<EgressViolation>, _> =
                        events.iter().map(|payload| serde_json::from_slice(payload)).collect();
                    return Ok(parsed?);
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err(format!("expected {count} violations").into())
    }
}

struct LoopbackLookup;

#[async_trait]
impl NameLookup for LoopbackLookup {
    async fn lookup(&self, _name: &str, _port: Port) -> io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
    }
}

fn policy() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: domain_pattern("allowed.test")?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![loopback_range()?],
        deny_ranges: vec![],
        denied_domains: vec!["blocked.test".parse()?],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    })
}

fn ca() -> TestResult<SandboxCa> {
    install_default_crypto_provider();
    Ok(SandboxCa::generate(PermittedDomains::from_rules(&[])?)?)
}

fn basic() -> String {
    format!("Basic {}", STANDARD.encode(format!("sandbox:{TOKEN}")))
}

fn reporter(sink: Arc<dyn ViolationSink>) -> Arc<ViolationReporter> {
    Arc::new(ViolationReporter::new(
        LeaseId::new("L1".to_string()),
        sink,
        ViolationRateLimit::default(),
    ))
}

async fn start(
    options: NetworkOptions,
    sink: Arc<dyn ViolationSink>,
    reasons: Option<DenialReasons>,
) -> TestResult<(ProxyServer, SocketAddr)> {
    start_gated(options, sink, reasons, None).await
}

async fn start_gated(
    options: NetworkOptions,
    sink: Arc<dyn ViolationSink>,
    reasons: Option<DenialReasons>,
    gate: Option<Arc<TunnelGate>>,
) -> TestResult<(ProxyServer, SocketAddr)> {
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let mut builder = ProxyServer::builder(policy()?, ca()?.permitting_every_name())
        .explicit(listener, ProxyToken::new(TOKEN)?)
        .violations(reporter(sink))
        .resolver(Arc::new(VettingResolver::new(
            Arc::new(LoopbackLookup),
            SsrfFloor::system(),
        )))
        .network_options(options);
    if let Some(reasons) = reasons {
        builder = builder.denial_reasons(reasons);
    }
    if let Some(gate) = gate {
        builder = builder.tunnel_gate(gate);
    }
    let server = builder.start()?;
    let addr = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(format!("{other:?}").into()),
    };
    Ok((server, addr))
}

async fn echo_upstream() -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    Ok(port)
}

async fn silent_upstream() -> TestResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    Ok(port)
}

async fn read_head<S: AsyncReadExt + Unpin>(stream: &mut S) -> TestResult<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            return Err(format!("closed early: {:?}", String::from_utf8_lossy(&buf)).into());
        }
        buf.push(byte[0]);
    }
    Ok(String::from_utf8(buf)?)
}

async fn connect_via(proxy: SocketAddr, target: &str) -> TestResult<(TcpStream, String)> {
    let mut stream = TcpStream::connect(proxy).await?;
    stream
        .write_all(
            format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {}\r\n\r\n",
                basic()
            )
            .as_bytes(),
        )
        .await?;
    let head = read_head(&mut stream).await?;
    Ok((stream, head))
}

async fn echo_roundtrip(stream: &mut TcpStream) -> TestResult {
    stream.write_all(b"ping").await?;
    let mut got = [0u8; 4];
    stream.read_exact(&mut got).await?;
    assert_eq!(&got, b"ping");
    Ok(())
}

fn reasons_for_blocked(reason: &str) -> TestResult<DeniedDomainReasons> {
    Ok(DeniedDomainReasons::new([(
        "blocked.test".parse::<DeniedDomain>()?,
        DenyReason::new(reason)?,
    )]))
}

#[tokio::test]
async fn a_denied_domain_reason_is_the_403_phrase_and_the_violation_line() -> TestResult {
    let store = ViolationStore::new();
    let reasons = DenialReasons::new();
    let sink = Arc::new(StoreViolationSink::new(
        store.clone(),
        CommandId::new("c1"),
        reasons.clone(),
    ));
    let options = NetworkOptions {
        denied_domain_reasons: reasons_for_blocked("blocked by corporate policy")?,
        ..NetworkOptions::default()
    };
    let (server, proxy) = start(options, sink, Some(reasons)).await?;

    let (_stream, head) = connect_via(proxy, "blocked.test:443").await?;
    assert!(
        head.starts_with("HTTP/1.1 403 blocked by corporate policy\r\n"),
        "{head}"
    );
    for _ in 0..100 {
        if store.count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let lines = store.violations(None);
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].line.as_str().contains("blocked by corporate policy"),
        "{}",
        lines[0].line.as_str()
    );
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_non_ascii_denied_domain_reason_is_masked_in_the_403_phrase_and_kept_in_the_violation() -> TestResult {
    let store = ViolationStore::new();
    let reasons = DenialReasons::new();
    let sink = Arc::new(StoreViolationSink::new(
        store.clone(),
        CommandId::new("c1"),
        reasons.clone(),
    ));
    let options = NetworkOptions {
        denied_domain_reasons: reasons_for_blocked("refused: caf\u{e9} \u{2603}")?,
        ..NetworkOptions::default()
    };
    let (server, proxy) = start(options, sink, Some(reasons)).await?;

    let (_stream, head) = connect_via(proxy, "blocked.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 refused: caf? ?\r\n"), "{head}");
    for _ in 0..100 {
        if store.count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let lines = store.violations(None);
    assert!(
        lines[0].line.as_str().contains("refused: caf\u{e9} \u{2603}"),
        "{}",
        lines[0].line.as_str()
    );
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_socks_client_to_a_denied_host_on_port_22_reads_the_reason_as_ssh() -> TestResult {
    let store = ViolationStore::new();
    let reasons = DenialReasons::new();
    let sink = Arc::new(StoreViolationSink::new(
        store.clone(),
        CommandId::new("c1"),
        reasons.clone(),
    ));
    let reason = "ssh to this host is blocked; use an https remote";
    let options = NetworkOptions {
        denied_domain_reasons: reasons_for_blocked(reason)?,
        ..NetworkOptions::default()
    };
    let (server, proxy) = start(options, sink, Some(reasons)).await?;

    let mut stream = TcpStream::connect(proxy).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let host = b"blocked.test";
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host);
    request.extend_from_slice(&22u16.to_be_bytes());
    stream.write_all(&request).await?;
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await?;

    assert_eq!(&out[..2], &[5, 0]);
    assert_eq!(&out[2..4], &[5, 0]);
    let ssh = &out[12..];
    let text = String::from_utf8_lossy(ssh);
    assert!(text.starts_with("SSH-2.0-"), "{text}");
    assert!(text.contains(reason), "{text}");
    let packet = &ssh[ssh.iter().position(|&b| b == b'\n').ok_or("no banner line")? + 1..];
    let length = u32::from_be_bytes(packet[..4].try_into()?) as usize;
    assert_eq!((4 + length) % 8, 0);
    assert_eq!(packet[5], 1);
    assert_eq!(u32::from_be_bytes(packet[6..10].try_into()?), 1);

    for _ in 0..100 {
        if store.count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let lines = store.violations(None);
    assert!(
        lines.iter().any(|v| v.line.as_str().contains("blocked.test:22")),
        "{lines:?}"
    );
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

async fn denied_connect_lines(
    rules: &[(&str, &[&str])],
    command_text: Option<&str>,
) -> TestResult<(String, Vec<String>)> {
    let store = ViolationStore::new();
    let sink = Arc::new(
        StoreViolationSink::new(store.clone(), CommandId::new("c1"), DenialReasons::new()).with_filter(
            ViolationFilter::new(
                Arc::new(IgnoreViolations::new(
                    rules
                        .iter()
                        .map(|(key, patterns)| (key.to_string(), patterns.iter().map(|p| p.to_string()).collect()))
                        .collect(),
                )),
                command_text.map(str::to_string),
            ),
        ),
    );
    let (server, proxy) = start(NetworkOptions::default(), sink, None).await?;
    let (_stream, head) = connect_via(proxy, "blocked.test:443").await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let lines = store
        .violations(None)
        .iter()
        .map(|violation| violation.line.as_str().to_string())
        .collect();
    server.shutdown(Duration::from_millis(100)).await?;
    Ok((head, lines))
}

#[tokio::test]
async fn an_ignored_denial_still_answers_403_while_its_violation_is_dropped() -> TestResult {
    let (head, lines) = denied_connect_lines(&[("*", &["blocked.test"])], None).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert!(lines.is_empty(), "{lines:?}");

    let (head, lines) = denied_connect_lines(&[("*", &["other.test"])], None).await?;
    assert!(head.starts_with("HTTP/1.1 403 "), "{head}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    Ok(())
}

#[tokio::test]
async fn a_command_pattern_matches_the_command_text_and_not_the_opaque_id() -> TestResult {
    let rules: &[(&str, &[&str])] = &[("curl", &["blocked.test"])];
    let (_, lines) = denied_connect_lines(rules, Some("curl https://blocked.test")).await?;
    assert!(lines.is_empty(), "{lines:?}");
    let (_, lines) = denied_connect_lines(rules, Some("wget https://blocked.test")).await?;
    assert_eq!(lines.len(), 1, "{lines:?}");
    let (_, lines) = denied_connect_lines(&[("c1", &["blocked.test"])], Some("wget x")).await?;
    assert_eq!(lines.len(), 1, "{lines:?}");
    Ok(())
}

#[tokio::test]
async fn without_reasons_the_403_phrase_stays_forbidden() -> TestResult {
    let sink = Arc::new(InMemorySink::default());
    let (server, proxy) = start(NetworkOptions::default(), sink, None).await?;
    let (_stream, head) = connect_via(proxy, "blocked.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{head}");
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[path = "network_options/tunnel_cap_tests.rs"]
mod tunnel_cap_tests;

#[tokio::test]
async fn plain_http_responses_lose_the_network_wide_stripped_headers() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nX-Powered-By: secret-stack\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    let strip = ResponseHeaderStrip::new([StrippedHeader::new("x-powered-by")?]);
    let options = NetworkOptions {
        strip_response_headers: strip,
        ..NetworkOptions::default()
    };
    let (server, proxy) = start(options, Arc::new(InMemorySink::default()), None).await?;

    let mut stream = TcpStream::connect(proxy).await?;
    stream
        .write_all(
            format!(
                "GET http://127.0.0.1:{upstream}/ HTTP/1.1\r\nHost: 127.0.0.1:{upstream}\r\nProxy-Authorization: {}\r\n\r\n",
                basic()
            )
            .as_bytes(),
        ).await?;
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    let response = String::from_utf8_lossy(&response).to_lowercase();
    assert!(response.starts_with("http/1.1 200 "), "{response}");
    assert!(!response.contains("x-powered-by"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_tunnel_chains_through_the_parent_proxy_with_the_original_hostname() -> TestResult {
    let parent = TcpListener::bind("127.0.0.1:0").await?;
    let parent_port = parent.local_addr()?.port();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = parent.accept().await else {
                break;
            };
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let Ok(head) = read_head(&mut stream).await else {
                    return;
                };
                locked(&recorded).push(head);
                if stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                let mut buf = [0u8; 1024];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let parent_proxy = ParentProxy::new(
        Some(ParentProxyUrl::new(&format!("http://127.0.0.1:{parent_port}"))?),
        None,
        NoProxy::new(""),
    )?;
    let options = NetworkOptions {
        parent_proxy: Some(parent_proxy),
        ..NetworkOptions::default()
    };
    let (server, proxy) = start(options, Arc::new(InMemorySink::default()), None).await?;

    let (mut tunnel, head) = connect_via(proxy, "allowed.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut tunnel).await?;
    let heads = locked(&seen).clone();
    assert_eq!(heads.len(), 1);
    assert!(
        heads[0].starts_with("CONNECT allowed.test:443 HTTP/1.1\r\n"),
        "{}",
        heads[0]
    );
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn extra_upstream_ca_certificates_are_trusted_and_the_default_is_not() -> TestResult {
    install_default_crypto_provider();
    let rule = DomainRule {
        domain: domain_pattern("localhost")?,
        action: DomainRuleAction::PassThrough,
    };
    let upstream_ca = SandboxCa::generate(PermittedDomains::from_rules(&[rule])?)?;
    let leaf = upstream_ca.mint_leaf_cert("localhost")?;
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config_for_leaf(leaf));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(stream).await {
                    let _ = tls.write_all(b"hi").await;
                    let _ = tls.shutdown().await;
                }
            });
        }
    });

    let connect = |config: Arc<rustls::ClientConfig>| async move {
        let stream = TcpStream::connect(("127.0.0.1", port)).await?;
        let name = rustls::pki_types::ServerName::try_from("localhost")
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        tokio_rustls::TlsConnector::from(config).connect(name, stream).await
    };

    let trusting = upstream_tls_config_trusting(&UpstreamCaCertificates::new([upstream_ca.root_cert_pem()]))?;
    let mut tls = connect(trusting).await?;
    let mut got = Vec::new();
    let _ = tls.read_to_end(&mut got).await;
    assert_eq!(got, b"hi");

    assert!(
        connect(trogon_sandbox_egress_proxy::server::system_upstream_tls_config())
            .await
            .is_err()
    );
    assert!(upstream_tls_config_trusting(&UpstreamCaCertificates::new(["not a pem".to_string()])).is_err());
    Ok(())
}

async fn start_custom(
    policy: NetworkPolicy,
    ca: SandboxCa,
    options: NetworkOptions,
    sink: Arc<dyn ViolationSink>,
    lookup: bool,
) -> io::Result<(ProxyServer, SocketAddr)> {
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let token = ProxyToken::new(TOKEN).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut builder = ProxyServer::builder(policy, ca)
        .explicit(listener, token)
        .violations(reporter(sink))
        .network_options(options);
    if lookup {
        builder = builder.resolver(Arc::new(VettingResolver::new(
            Arc::new(LoopbackLookup),
            SsrfFloor::system(),
        )));
    }
    let server = builder.start()?;
    let addr = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(io::Error::other(format!("{other:?}"))),
    };
    Ok((server, addr))
}

fn pass_through(domains: &[&str], allow_loopback: bool) -> TestResult<NetworkPolicy> {
    let rules = domains
        .iter()
        .map(|domain| {
            Ok(DomainRule {
                domain: domain_pattern(domain)?,
                action: DomainRuleAction::PassThrough,
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    Ok(NetworkPolicy::UserDefined {
        rules,
        allow_ranges: if allow_loopback {
            vec![loopback_range()?]
        } else {
            vec![]
        },
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    })
}

#[cfg(unix)]
#[tokio::test]
async fn a_mitm_domain_is_tunnelled_through_the_unix_socket() -> TestResult {
    let dir = std::env::temp_dir().join(format!("mitm-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket = dir.join("m.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let seen = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let head = read_head(&mut stream)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await?;
        stream.write_all(&buf).await?;
        Ok::<String, io::Error>(head)
    });
    let options = NetworkOptions {
        mitm: Some(MitmUnixSocket {
            socket_path: socket.clone(),
            domains: vec![DomainPattern::new("allowed.test")?],
        }),
        ..NetworkOptions::default()
    };
    let (server, proxy) = start(options, Arc::new(InMemorySink::default()), None).await?;

    let (mut tunnel, head) = connect_via(proxy, "allowed.test:443").await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut tunnel).await?;
    assert!(
        seen.await?
            .map_err(|error| error.to_string())?
            .starts_with("CONNECT allowed.test:443 HTTP/1.1\r\n")
    );
    server.shutdown(Duration::from_millis(100)).await?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test]
async fn loopback_names_reach_loopback_only_when_the_option_is_on() -> TestResult {
    let upstream = echo_upstream().await?;
    if let Ok(v6) = TcpListener::bind(("::1", upstream)).await {
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = v6.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
    }
    let policy = pass_through(&["localhost"], false)?;

    let sink = Arc::new(InMemorySink::default());
    let (off, proxy) = start_custom(policy.clone(), ca()?, NetworkOptions::default(), sink.clone(), false).await?;
    let mut stream = TcpStream::connect(proxy).await?;
    stream
        .write_all(
            format!(
                "CONNECT localhost:{upstream} HTTP/1.1\r\nHost: localhost:{upstream}\r\nProxy-Authorization: {}\r\n\r\n",
                basic()
            )
            .as_bytes(),
        ).await?;
    let mut reply = Vec::new();
    let _ = stream.read_to_end(&mut reply).await;
    assert!(!String::from_utf8_lossy(&reply).starts_with("HTTP/1.1 200 "));
    off.shutdown(Duration::from_millis(100)).await?;

    let (on, proxy) = start_custom(
        policy,
        ca()?,
        NetworkOptions {
            loopback_names: LoopbackNames::from(true),
            ..NetworkOptions::default()
        },
        sink,
        false,
    )
    .await?;
    let (mut tunnel, head) = connect_via(proxy, &format!("localhost:{upstream}")).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    echo_roundtrip(&mut tunnel).await?;
    on.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn loopback_names_are_refused_together_with_a_custom_resolver() -> TestResult {
    let result = start_custom(
        pass_through(&["localhost"], true)?,
        ca()?,
        NetworkOptions {
            loopback_names: LoopbackNames::from(true),
            ..NetworkOptions::default()
        },
        Arc::new(InMemorySink::default()),
        true,
    )
    .await;
    assert!(result.is_err());
    Ok(())
}

async fn read_http_response<S: AsyncReadExt + Unpin>(stream: &mut S) -> TestResult<String> {
    let head = read_head(stream).await?;
    let length = match head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
    {
        Some((_, value)) => value.trim().parse::<usize>()?,
        None => 0,
    };
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).await?;
    Ok(format!("{head}{}", String::from_utf8(body)?))
}

#[tokio::test]
async fn terminated_tunnels_lose_the_network_wide_stripped_headers() -> TestResult {
    let rules = vec![DomainRule {
        domain: domain_pattern("tls.test")?,
        action: DomainRuleAction::Forward(RequestForward {
            forward_url: ForwardUrl::new("https://forward.invalid/hook")?,
            match_rules: vec![MatchRule {
                path: Some(StringMatcher::Exact("/never".to_string())),
                methods: Vec::new(),
                query_string: Vec::new(),
                headers: Vec::new(),
            }],
            ttl_seconds: RequestForward::DEFAULT_TTL_SECONDS,
        }),
    }];
    install_default_crypto_provider();
    let upstream_ca = SandboxCa::generate(PermittedDomains::from_rules(&[DomainRule {
        domain: domain_pattern("tls.test")?,
        action: DomainRuleAction::PassThrough,
    }])?)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config_for_leaf(upstream_ca.mint_leaf_cert("tls.test")?));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let _ = read_head(&mut tls).await;
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nX-Powered-By: secret-stack\r\nX-Kept: yes\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });

    let sandbox_ca = SandboxCa::generate(PermittedDomains::from_rules(&rules)?)?;
    let options = NetworkOptions {
        upstream_ca: UpstreamCaCertificates::new([upstream_ca.root_cert_pem()]),
        strip_response_headers: ResponseHeaderStrip::new([StrippedHeader::new("x-powered-by")?]),
        ..NetworkOptions::default()
    };
    let policy = NetworkPolicy::UserDefined {
        rules,
        allow_ranges: vec![loopback_range()?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    };
    let (server, proxy) = start_custom(policy, sandbox_ca, options, Arc::new(InMemorySink::default()), true).await?;

    let (tunnel, head) = connect_via(proxy, &format!("tls.test:{port}")).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut io::Cursor::new(server.ca_certificate_pem().as_bytes().to_vec())) {
        roots.add(cert?)?;
    }
    let config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let mut tls = tokio_rustls::TlsConnector::from(config)
        .connect(rustls::pki_types::ServerName::try_from("tls.test")?, tunnel)
        .await?;
    tls.write_all(format!("GET /health HTTP/1.1\r\nHost: tls.test:{port}\r\n\r\n").as_bytes())
        .await?;
    let response = read_http_response(&mut tls).await?.to_lowercase();
    assert!(response.starts_with("http/1.1 200 "), "{response}");
    assert!(!response.contains("x-powered-by"), "{response}");
    assert!(response.contains("x-kept: yes"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

fn gate(cap: u32) -> TestResult<Arc<TunnelGate>> {
    Ok(Arc::new(TunnelGate::new(Some(TunnelCap::new(cap)?), None)))
}

#[tokio::test]
async fn proxies_sharing_a_gate_share_the_terminated_tunnel_count() -> TestResult {
    let upstream = silent_upstream().await?;
    let shared = gate(1)?;
    let options = terminating;
    let (first_server, first_proxy) =
        start_gated(options(), Arc::new(InMemorySink::default()), None, Some(shared.clone())).await?;
    let (second_server, second_proxy) =
        start_gated(options(), Arc::new(InMemorySink::default()), None, Some(shared.clone())).await?;
    let target = format!("127.0.0.1:{upstream}");

    let (_held, head) = connect_via(first_proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    let (_refused, head) = connect_via(second_proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 503 "), "{head}");
    first_server.shutdown(Duration::from_millis(100)).await?;
    second_server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_cap_changed_on_the_gate_applies_to_the_next_tunnel() -> TestResult {
    let upstream = silent_upstream().await?;
    let shared = gate(1)?;
    let (server, proxy) = start_gated(
        terminating(),
        Arc::new(InMemorySink::default()),
        None,
        Some(shared.clone()),
    )
    .await?;
    let target = format!("127.0.0.1:{upstream}");

    let (_first, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    let (_refused, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 503 "), "{head}");

    shared.set_cap(Some(TunnelCap::new(2)?));
    let (_second, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn the_handshake_timeout_counts_from_the_connect() -> TestResult {
    let upstream = silent_upstream().await?;
    let shared = Arc::new(TunnelGate::new(None, Some(Duration::from_millis(400))));
    let (server, proxy) = start_gated(terminating(), Arc::new(InMemorySink::default()), None, Some(shared)).await?;
    let target = format!("127.0.0.1:{upstream}");

    let started = tokio::time::Instant::now();
    let (mut client, head) = connect_via(proxy, &target).await?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    tokio::time::sleep(Duration::from_millis(250)).await;
    client.write_all(&[0x16, 0x03, 0x01, 0x00, 0xff, 0x01]).await?;
    let mut rest = Vec::new();
    let _ = client.read_to_end(&mut rest).await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(380), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(600), "{elapsed:?}");
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}
