use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use trogon_sandbox_contract::UpstreamCaCertificates;
use trogon_sandbox_contract::{
    AllowRange, AllowedTlsTermination, DomainRule, DomainRuleAction, NetworkOptions, NetworkPolicy, TlsExclusions,
};
use trogon_sandbox_egress_proxy::deny_header_style::DenyHeaderStyle;
use trogon_sandbox_egress_proxy::ingress::{IngressLimits, ListenAddress, Listener};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer, upstream_tls_config_trusting};
use trogon_sandbox_egress_proxy::tls_ca::{
    PermittedDomains, SandboxCa, install_default_crypto_provider, server_config_for_leaf,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn loopback_range() -> TestResult<AllowRange> {
    Ok(AllowRange::new("127.0.0.0/8")?)
}

fn terminating() -> NetworkOptions {
    NetworkOptions {
        terminate_allowed_tls: AllowedTlsTermination::from(true),
        ..NetworkOptions::default()
    }
}

fn policy() -> TestResult<NetworkPolicy> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![loopback_range()?],
        deny_ranges: vec![],
        denied_domains: vec!["blocked.test".parse()?],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn ca() -> TestResult<SandboxCa> {
    install_default_crypto_provider();
    Ok(SandboxCa::generate(PermittedDomains::from_rules(&[])?)?)
}

async fn start_unauthenticated(
    ca: SandboxCa,
    options: NetworkOptions,
    style: Option<DenyHeaderStyle>,
) -> TestResult<(ProxyServer, SocketAddr)> {
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let mut builder = ProxyServer::builder(policy()?, ca)
        .explicit_unauthenticated(listener)
        .network_options(options);
    if let Some(style) = style {
        builder = builder.deny_header_style(style);
    }
    let server = builder.start()?;
    let addr = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(format!("{other:?}").into()),
    };
    Ok((server, addr))
}

async fn answer_to(proxy: SocketAddr, request: &[u8]) -> TestResult<String> {
    let mut stream = TcpStream::connect(proxy).await?;
    stream.write_all(request).await?;
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

#[tokio::test]
async fn servers_in_one_process_name_refusals_in_their_own_style() -> TestResult {
    let (default_style, default_addr) = start_unauthenticated(ca()?, NetworkOptions::default(), None).await?;
    let (reasons, reasons_addr) =
        start_unauthenticated(ca()?, NetworkOptions::default(), Some(DenyHeaderStyle::DenyReason)).await?;
    let connect = b"CONNECT blocked.test:443 HTTP/1.1\r\nHost: blocked.test:443\r\n\r\n";

    let first = answer_to(default_addr, connect).await?;
    let second = answer_to(reasons_addr, connect).await?;
    let first_again = answer_to(default_addr, connect).await?;

    assert!(first.contains("X-Proxy-Error: "), "{first}");
    assert!(!first.contains("X-Deny-Reason: "), "{first}");
    assert!(second.contains("X-Deny-Reason: host_not_allowed"), "{second}");
    assert!(!second.contains("X-Proxy-Error: "), "{second}");
    assert!(first_again.contains("X-Proxy-Error: "), "{first_again}");
    default_style.shutdown(Duration::from_millis(100)).await?;
    reasons.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_listener_answers_socks_and_unknown_bytes_with_a_400() -> TestResult {
    let (server, addr) =
        start_unauthenticated(ca()?, NetworkOptions::default(), Some(DenyHeaderStyle::DenyReason)).await?;
    for first in [
        &b"\x05\x01\x00"[..],
        &b"\x04\x01\x00\x50\x7f\x00\x00\x01\x00"[..],
        &b"\x16\x03\x01\x00\x00"[..],
    ] {
        let response = answer_to(addr, first).await?;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{response:?}");
        assert!(response.contains("X-Deny-Reason: bad_request\r\n"), "{response:?}");
    }
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn an_open_listener_serves_socks5_without_a_login() -> TestResult {
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy()?, ca()?).explicit_open(listener).start()?;
    let Some(BoundAddress::Tcp(addr)) = server.explicit_addr() else {
        return Err("not a tcp listener".into());
    };

    let mut stream = TcpStream::connect(addr).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).await?;
    assert_eq!(method, [5, 0]);
    let host = b"blocked.test";
    let mut connect = vec![5, 1, 0, 3, host.len() as u8];
    connect.extend_from_slice(host);
    connect.extend_from_slice(&443u16.to_be_bytes());
    stream.write_all(&connect).await?;
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).await?;
    assert_eq!(reply[..2], [5, 2]);

    let mut login_only = TcpStream::connect(addr).await?;
    login_only.write_all(&[5, 1, 2]).await?;
    let mut refused = [0u8; 2];
    login_only.read_exact(&mut refused).await?;
    assert_eq!(refused, [5, 0xFF]);
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn an_ip_literal_connect_is_decided_and_terminated_when_every_host_is_terminated() -> TestResult {
    install_default_crypto_provider();
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    tokio::spawn(async move { while let Ok((_stream, _)) = upstream.accept().await {} });
    let ca = ca()?.permitting_every_name();
    let root = ca.root_cert_pem();
    let (server, addr) = start_unauthenticated(ca, terminating(), None).await?;

    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
        .await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).await?, 1);
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");

    let config = upstream_tls_config_trusting(&UpstreamCaCertificates::new([root]))?;
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1")?;
    let handshake = tokio_rustls::TlsConnector::from(config).connect(name, stream);
    let tls = tokio::time::timeout(Duration::from_secs(5), handshake).await?;
    assert!(tls.is_ok(), "{:?}", tls.err());
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn an_ip_literal_named_by_an_exclusion_is_relayed_unterminated() -> TestResult {
    install_default_crypto_provider();
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let received = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await?;
        let mut hello = [0u8; 9];
        stream.read_exact(&mut hello).await?;
        Ok::<[u8; 9], std::io::Error>(hello)
    });
    let options = NetworkOptions {
        tls_exclusions: TlsExclusions::new(["127.0.0.1".parse()?]),
        ..terminating()
    };
    let (server, addr) = start_unauthenticated(ca()?.permitting_every_name(), options, None).await?;

    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
        .await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).await?, 1);
        head.push(byte[0]);
    }
    let hello = [0x16, 3, 1, 0, 4, 1, 2, 3, 4];
    stream.write_all(&hello).await?;
    let arrived = tokio::time::timeout(Duration::from_secs(5), received).await???;
    assert_eq!(arrived, hello);
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

async fn with_deny_reasons() -> TestResult<(ProxyServer, SocketAddr)> {
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy()?, ca()?)
        .explicit_unauthenticated(listener)
        .ingress_limits(IngressLimits::proxy_only())
        .deny_header_style(DenyHeaderStyle::DenyReason)
        .start()?;
    let addr = match server.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        other => return Err(format!("{other:?}").into()),
    };
    Ok((server, addr))
}

#[tokio::test]
async fn the_plain_path_answers_421_with_the_misdirected_mark() -> TestResult {
    let (server, addr) = with_deny_reasons().await?;
    let response = answer_to(
        addr,
        b"GET http://127.0.0.1:9/ok HTTP/1.1\r\nHost: other.example.test\r\nConnection: close\r\n\r\n",
    )
    .await?;
    assert!(response.starts_with("HTTP/1.1 421 "), "{response:?}");
    assert!(response.contains("X-Deny-Reason: misdirected\r\n"), "{response:?}");
    assert!(!response.to_ascii_lowercase().contains("x-proxy-error"));
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_destination_host_that_is_not_plain_ascii_is_answered_400_bad_request() -> TestResult {
    let (server, addr) = with_deny_reasons().await?;
    for target in [
        "\u{ff41}llowed.test:443",
        "allowed%2etest:443",
        "allowed.test..:443",
        ".allowed.test:443",
        "allowed..test:443",
        "allow\u{e9}d.test:443",
    ] {
        let request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
        let response = answer_to(addr, request.as_bytes()).await?;
        assert!(response.starts_with("HTTP/1.1 400 "), "{target}: {response:?}");
        assert!(
            response.contains("X-Deny-Reason: bad_request\r\n"),
            "{target}: {response:?}"
        );
    }
    for url in [
        "http://\u{ff41}llowed.test/x",
        "http://allowed%2etest/x",
        "http://allowed.test../x",
    ] {
        let request = format!("GET {url} HTTP/1.1\r\nHost: allowed.test\r\nConnection: close\r\n\r\n");
        let response = answer_to(addr, request.as_bytes()).await?;
        assert!(response.starts_with("HTTP/1.1 400 "), "{url}: {response:?}");
        assert!(
            response.contains("X-Deny-Reason: bad_request\r\n"),
            "{url}: {response:?}"
        );
    }
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

const EVENTS: usize = 5;

async fn serve_events<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    sent: Arc<AtomicUsize>,
    mut acks: tokio::sync::mpsc::UnboundedReceiver<()>,
) -> std::io::Result<()> {
    let mut byte = [0u8; 1];
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if !matches!(stream.read(&mut byte).await, Ok(1)) {
            return Ok(());
        }
        head.push(byte[0]);
    }
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
        .await?;
    stream.flush().await?;
    for n in 1..=EVENTS {
        sent.store(n, Ordering::SeqCst);
        stream.write_all(format!("data: {n}\n\n").as_bytes()).await?;
        stream.flush().await?;
        let acked = tokio::time::timeout(Duration::from_secs(5), acks.recv()).await;
        if !matches!(acked, Ok(Some(()))) {
            break;
        }
    }
    let _ = stream.shutdown().await;
    Ok(())
}

async fn arrivals<S: AsyncRead + Unpin>(
    mut stream: S,
    sent: &AtomicUsize,
    acks: &tokio::sync::mpsc::UnboundedSender<()>,
) -> Vec<usize> {
    let mut seen = 0;
    let mut text = String::new();
    let mut sent_at_arrival = Vec::new();
    let mut buffer = [0u8; 1024];
    while let Ok(Ok(read)) = tokio::time::timeout(Duration::from_secs(8), stream.read(&mut buffer)).await {
        if read == 0 {
            break;
        }
        text.push_str(&String::from_utf8_lossy(&buffer[..read]));
        let events = text.matches("\n\n").count();
        while seen < events {
            seen += 1;
            sent_at_arrival.push(sent.load(Ordering::SeqCst));
            let _ = acks.send(());
        }
    }
    sent_at_arrival
}

#[tokio::test]
async fn each_server_sent_event_reaches_the_client_before_the_next_is_sent_over_a_terminated_tunnel() -> TestResult {
    install_default_crypto_provider();
    let upstream_ca = ca()?.permitting_every_name();
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config_for_leaf(upstream_ca.mint_leaf_cert("127.0.0.1")?));
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let sent = Arc::new(AtomicUsize::new(0));
    let (ack_tx, ack_rx) = tokio::sync::mpsc::unbounded_channel();
    let upstream_sent = sent.clone();
    tokio::spawn(async move {
        let Ok((stream, _)) = upstream.accept().await else {
            return;
        };
        let Ok(tls) = acceptor.accept(stream).await else {
            return;
        };
        let _ = serve_events(tls, upstream_sent, ack_rx).await;
    });
    let proxy_ca = ca()?.permitting_every_name();
    let root = proxy_ca.root_cert_pem();
    let options = NetworkOptions {
        upstream_ca: UpstreamCaCertificates::new([upstream_ca.root_cert_pem()]),
        ..terminating()
    };
    let (server, addr) = start_unauthenticated(proxy_ca, options, None).await?;

    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
        .await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).await?, 1);
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 200 "));
    let config = upstream_tls_config_trusting(&UpstreamCaCertificates::new([root]))?;
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1")?;
    let mut tls = tokio_rustls::TlsConnector::from(config).connect(name, stream).await?;
    tls.write_all(format!("GET /events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
        .await?;

    let order = arrivals(tls, &sent, &ack_tx).await;
    assert_eq!(order, (1..=EVENTS).collect::<Vec<_>>());
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn each_server_sent_event_reaches_the_client_before_the_next_is_sent_over_the_plain_path() -> TestResult {
    install_default_crypto_provider();
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let sent = Arc::new(AtomicUsize::new(0));
    let (ack_tx, ack_rx) = tokio::sync::mpsc::unbounded_channel();
    let upstream_sent = sent.clone();
    tokio::spawn(async move {
        let Ok((stream, _)) = upstream.accept().await else {
            return;
        };
        let _ = serve_events(stream, upstream_sent, ack_rx).await;
    });
    let (server, addr) = start_unauthenticated(ca()?, NetworkOptions::default(), None).await?;

    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(
            format!(
                "GET http://127.0.0.1:{port}/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;

    let order = arrivals(stream, &sent, &ack_tx).await;
    assert_eq!(order, (1..=EVENTS).collect::<Vec<_>>());
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_connect_to_a_denied_port_is_refused_even_when_an_allowed_entry_covers_it() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: "denied.test".parse()?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![loopback_range()?],
        deny_ranges: vec![],
        denied_domains: vec!["denied.test:443".parse()?],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let server = ProxyServer::builder(policy, ca()?)
        .explicit_unauthenticated(listener)
        .ingress_limits(IngressLimits::proxy_only())
        .deny_header_style(DenyHeaderStyle::DenyReason)
        .start()?;
    let Some(BoundAddress::Tcp(addr)) = server.explicit_addr() else {
        return Err("not a tcp listener".into());
    };

    let response = answer_to(
        *addr,
        b"CONNECT denied.test:443 HTTP/1.1\r\nHost: denied.test:443\r\n\r\n",
    )
    .await?;
    assert!(response.starts_with("HTTP/1.1 403 "), "{response:?}");
    assert!(response.contains("X-Deny-Reason: host_not_allowed\r\n"), "{response:?}");
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}

#[tokio::test]
async fn a_connect_and_a_bad_head_are_refused_with_bare_status_lines() -> TestResult {
    let closed = TcpListener::bind("127.0.0.1:0").await?;
    let port = closed.local_addr()?.port();
    drop(closed);
    let (server, addr) = with_deny_reasons().await?;
    let bad_target = answer_to(addr, b"CONNECT no-port HTTP/1.1\r\nHost: no-port\r\n\r\n").await?;
    assert_eq!(
        bad_target,
        "HTTP/1.1 400 Bad Request\r\nX-Deny-Reason: bad_request\r\n\r\n"
    );
    let target = format!("127.0.0.1:{port}");
    let unreachable = answer_to(
        addr,
        format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes(),
    )
    .await?;
    assert_eq!(unreachable, "HTTP/1.1 502 Bad Gateway\r\n\r\n");
    let unknown = answer_to(addr, &[0x16, 0x03, 0x01]).await?;
    assert_eq!(
        unknown,
        "HTTP/1.1 400 Bad Request\r\nX-Deny-Reason: bad_request\r\n\r\n"
    );
    server.shutdown(Duration::from_millis(100)).await?;
    Ok(())
}
