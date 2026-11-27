use super::*;
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::TcpListener;
#[cfg(unix)]
use trogon_sandbox_contract::{DomainPattern, MitmUnixSocket};
use trogon_sandbox_contract::{NoProxy, ParentProxy};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn invalid_data(error: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(target_os = "linux")]
type Accepted = tokio::task::JoinHandle<io::Result<TcpStream>>;

#[cfg(target_os = "linux")]
async fn listening() -> Result<(SocketAddr, Accepted), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    Ok((
        addr,
        tokio::spawn(async move { listener.accept().await.map(|(stream, _)| stream) }),
    ))
}

#[cfg(target_os = "linux")]
fn mark_of(stream: &TcpStream) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(socket2::SockRef::from(stream).mark()?)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn required_marking_marks_the_dialed_socket() -> TestResult {
    let (addr, accept) = listening().await?;
    let stream = dial(UpstreamMarking::Required, addr).await?;
    assert_eq!(mark_of(&stream)?, UPSTREAM_MARK.get());
    accept.await??;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn not_needed_marking_leaves_the_socket_unmarked() -> TestResult {
    let (addr, accept) = listening().await?;
    let stream = dial(UpstreamMarking::NotNeeded, addr).await?;
    assert_eq!(mark_of(&stream)?, 0);
    accept.await??;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn declared_destinations_follow_the_marking() -> TestResult {
    let (addr, accept) = listening().await?;
    let stream = connect_declared(UpstreamMarking::Required, "127.0.0.1", addr.port()).await?;
    assert_eq!(mark_of(&stream)?, UPSTREAM_MARK.get());
    accept.await??;

    let (addr, accept) = listening().await?;
    let stream = connect_declared(UpstreamMarking::NotNeeded, "127.0.0.1", addr.port()).await?;
    assert_eq!(mark_of(&stream)?, 0);
    accept.await??;
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn required_marking_verifies_with_net_admin() -> TestResult {
    UpstreamMarking::Required.verify()?;
    Ok(())
}

#[test]
fn not_needed_marking_never_probes() -> TestResult {
    UpstreamMarking::NotNeeded.verify()?;
    Ok(())
}

#[test]
fn marking_follows_the_transparent_listener() -> TestResult {
    assert_eq!(
        UpstreamMarking::for_transparent_listener(true),
        UpstreamMarking::Required
    );
    assert_eq!(
        UpstreamMarking::for_transparent_listener(false),
        UpstreamMarking::NotNeeded
    );
    Ok(())
}

#[test]
fn no_exempt_uid_selects_mark_exemption() -> TestResult {
    assert_eq!(UpstreamExemption::from_exempt_uid(None)?, UpstreamExemption::Mark);
    Ok(())
}

#[test]
fn an_exempt_uid_selects_uid_exemption() -> TestResult {
    assert_eq!(
        UpstreamExemption::from_exempt_uid(Some("65531"))?,
        UpstreamExemption::Uid(ProxyUid::new(65531))
    );
    Ok(())
}

#[test]
fn a_malformed_exempt_uid_is_rejected() -> TestResult {
    for value in ["", "proxy", "-1", "4294967296"] {
        assert_eq!(
            UpstreamExemption::from_exempt_uid(Some(value)),
            Err(UpstreamExemptionError::InvalidUid {
                value: value.to_string()
            }),
            "{value:?}"
        );
    }
    Ok(())
}

#[test]
fn uid_exemption_never_marks_while_mark_exemption_follows_the_listener() -> TestResult {
    let by_uid = UpstreamExemption::Uid(ProxyUid::new(1));
    assert_eq!(
        by_uid.marking_for_transparent_listener(true),
        UpstreamMarking::NotNeeded
    );
    assert_eq!(
        UpstreamExemption::Mark.marking_for_transparent_listener(true),
        UpstreamMarking::Required
    );
    assert_eq!(
        UpstreamExemption::Mark.marking_for_transparent_listener(false),
        UpstreamMarking::NotNeeded
    );
    Ok(())
}

#[test]
fn uid_exemption_fails_closed_when_the_process_uid_differs() -> TestResult {
    let exempt = ProxyUid::new(65531);
    let by_uid = UpstreamExemption::Uid(exempt);
    assert_eq!(by_uid.verify_uid(exempt), Ok(()));
    assert_eq!(
        by_uid.verify_uid(ProxyUid::new(65532)),
        Err(UpstreamExemptionError::UidMismatch {
            exempt,
            actual: ProxyUid::new(65532)
        })
    );
    assert_eq!(UpstreamExemption::Mark.verify_uid(ProxyUid::new(0)), Ok(()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_effective_uid_matches_the_kernel() -> TestResult {
    assert_eq!(effective_uid()?.get(), unsafe { libc::geteuid() });
    Ok(())
}

fn pair() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    tokio::io::duplex(64 * 1024)
}

async fn read_head<S>(stream: &mut S) -> io::Result<String>
where
    S: AsyncRead + Unpin,
{
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
    }
    String::from_utf8(head).map_err(invalid_data)
}

async fn rejected(host: &str, port: u16) -> Result<io::Error, Box<dyn std::error::Error>> {
    let (mut client, _server) = pair();
    match open_connect_tunnel(&mut client, host, port, None).await {
        Err(error) => Ok(error),
        Ok(()) => Err("expected the tunnel to be rejected".into()),
    }
}

async fn failed_tunnel<S>(stream: &mut S, host: &str) -> Result<io::Error, Box<dyn std::error::Error>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match open_connect_tunnel(stream, host, 443, None).await {
        Err(error) => Ok(error),
        Ok(()) => Err("expected the tunnel to fail".into()),
    }
}

#[tokio::test]
async fn a_host_with_line_breaks_is_never_written() -> TestResult {
    for host in [
        "example.com\r\nX-Injected: 1",
        "example.com\nfoo",
        "a b",
        "evil.com HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\nHost: internal",
    ] {
        let (mut client, mut server) = pair();
        let err = failed_tunnel(&mut client, host).await?;
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{host:?}");
        drop(client);
        let mut written = Vec::new();
        server.read_to_end(&mut written).await?;
        assert!(written.is_empty(), "{host:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_invalid_port_is_rejected() -> TestResult {
    assert_eq!(rejected("example.com", 0).await?.kind(), io::ErrorKind::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn the_request_names_the_authority_and_carries_the_credentials() -> TestResult {
    let (mut client, mut server) = pair();
    let proxy = tokio::spawn(async move {
        let head = read_head(&mut server).await?;
        server
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\nearly")
            .await?;
        io::Result::Ok(head)
    });
    open_connect_tunnel(&mut client, "example.com", 443, Some("Basic dTpw")).await?;
    let head = proxy.await??;
    assert_eq!(
        head,
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Basic dTpw\r\n\r\n"
    );
    let mut early = [0u8; 5];
    client.read_exact(&mut early).await?;
    assert_eq!(&early, b"early");
    Ok(())
}

#[tokio::test]
async fn an_ipv6_literal_is_bracketed_in_the_authority() -> TestResult {
    let (mut client, mut server) = pair();
    let proxy = tokio::spawn(async move {
        let head = read_head(&mut server).await?;
        server.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await?;
        io::Result::Ok(head)
    });
    open_connect_tunnel(&mut client, "[2001:db8::1]", 8443, None).await?;
    assert!(
        proxy
            .await??
            .starts_with("CONNECT [2001:db8::1]:8443 HTTP/1.1\r\nHost: [2001:db8::1]:8443\r\n")
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the refusal is an io::Error whose message is its only distinguishing data"
    )
)]
async fn a_refusal_from_the_proxy_is_an_error() -> TestResult {
    let (mut client, mut server) = pair();
    let proxy = tokio::spawn(async move {
        read_head(&mut server).await?;
        server.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await?;
        io::Result::Ok(())
    });
    let err = failed_tunnel(&mut client, "example.com").await?;
    assert!(err.to_string().contains("403"), "{err}");
    proxy.await??;
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the refusal is an io::Error whose message is its only distinguishing data"
    )
)]
async fn an_oversized_response_head_is_an_error() -> TestResult {
    let (mut client, mut server) = pair();
    tokio::spawn(async move {
        let _ = read_head(&mut server).await;
        let _ = server.write_all(b"HTTP/1.1 200 OK\r\nX: ").await;
        let _ = server.write_all(&vec![b'a'; 20 * 1024]).await;
    });
    let err = failed_tunnel(&mut client, "example.com").await?;
    assert!(err.to_string().contains("too large"), "{err}");
    Ok(())
}

#[tokio::test]
async fn a_proxy_that_closes_early_is_an_error() -> TestResult {
    let (mut client, mut server) = pair();
    tokio::spawn(async move {
        let _ = read_head(&mut server).await;
    });
    let err = failed_tunnel(&mut client, "example.com").await?;
    assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    Ok(())
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, Box<dyn std::error::Error>> {
    Ok(Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    ))
}

fn endpoint(port: u16) -> VettedEndpoint {
    crate::resolve::VettedAddr::assume_vetted(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))).with_port(port)
}

fn loopback(port: u16) -> VettedEndpoint {
    crate::resolve::VettedAddr::assume_vetted(IpAddr::V4(Ipv4Addr::LOCALHOST)).with_port(port)
}

fn parent_options(
    http: Option<&str>,
    https: Option<&str>,
    no_proxy: &str,
) -> Result<NetworkOptions, Box<dyn std::error::Error>> {
    let http = http.map(ParentProxyUrl::new).transpose()?;
    let https = https.map(ParentProxyUrl::new).transpose()?;
    Ok(NetworkOptions {
        parent_proxy: Some(ParentProxy::new(http, https, NoProxy::new(no_proxy))?),
        ..NetworkOptions::default()
    })
}

async fn tunnelling_proxy(listener: TcpListener) -> tokio::task::JoinHandle<io::Result<String>> {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let head = read_head(&mut stream).await?;
        stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        stream.write_all(b"pong").await?;
        Ok(head)
    })
}

#[tokio::test]
async fn a_parent_proxy_receives_the_requested_name_in_connect() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_port = listener.local_addr()?.port();
    let seen = tunnelling_proxy(listener).await;
    let options = parent_options(None, Some(&format!("http://user:pw@127.0.0.1:{proxy_port}")), "")?;
    let config = tls_config()?;
    let routes = UpstreamRoutes {
        marking: UpstreamMarking::NotNeeded,
        options: &options,
        tls_config: &config,
        parent_pool: &ParentPool::default(),
    };
    let mut stream = routes.connect(Some("example.com"), true, endpoint(443)).await?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await?;
    assert_eq!(&reply, b"pong");
    let head = seen.await??;
    assert!(head.starts_with("CONNECT example.com:443 HTTP/1.1\r\n"), "{head}");
    assert!(head.contains("Proxy-Authorization: Basic dXNlcjpwdw==\r\n"), "{head}");
    Ok(())
}

#[tokio::test]
async fn no_proxy_and_loopback_skip_the_parent_proxy() -> TestResult {
    let direct = TcpListener::bind("127.0.0.1:0").await?;
    let direct_port = direct.local_addr()?.port();
    let options = parent_options(Some("http://127.0.0.1:1"), None, "internal.test")?;
    let config = tls_config()?;
    let routes = UpstreamRoutes {
        marking: UpstreamMarking::NotNeeded,
        options: &options,
        tls_config: &config,
        parent_pool: &ParentPool::default(),
    };
    let accepted = tokio::spawn(async move { direct.accept().await.is_ok() });
    routes
        .connect(Some("internal.test"), false, loopback(direct_port))
        .await?;
    assert!(accepted.await?);
    let direct = TcpListener::bind("127.0.0.1:0").await?;
    let direct_port = direct.local_addr()?.port();
    let accepted = tokio::spawn(async move { direct.accept().await.is_ok() });
    routes.connect(Some("localhost"), false, loopback(direct_port)).await?;
    assert!(accepted.await?);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn the_mitm_socket_wins_over_the_parent_proxy_for_its_domains() -> TestResult {
    let dir = std::env::temp_dir().join(format!("mitm-route-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let socket = dir.join("m.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let seen = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let head = read_head(&mut stream).await?;
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\nmitm").await?;
        io::Result::Ok(head)
    });
    let mut options = parent_options(Some("http://127.0.0.1:1"), None, "")?;
    options.mitm = Some(MitmUnixSocket {
        socket_path: socket.clone(),
        domains: vec![DomainPattern::new("*.corp.test")?],
    });
    let config = tls_config()?;
    let routes = UpstreamRoutes {
        marking: UpstreamMarking::NotNeeded,
        options: &options,
        tls_config: &config,
        parent_pool: &ParentPool::default(),
    };
    let mut stream = routes.connect(Some("api.corp.test"), true, endpoint(443)).await?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await?;
    assert_eq!(&reply, b"mitm");
    assert!(seen.await??.starts_with("CONNECT api.corp.test:443 HTTP/1.1\r\n"));
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[test]
fn required_marking_verifies_where_marking_is_not_applied() -> TestResult {
    UpstreamMarking::Required.verify()?;
    Ok(())
}

#[tokio::test]
async fn an_unresolved_destination_is_never_dialed() -> TestResult {
    let unresolved = crate::resolve::VettedAddr::unresolved(None).with_port(80);
    let err = connect(UpstreamMarking::NotNeeded, unresolved)
        .await
        .err()
        .ok_or("an unresolved destination was dialed")?;
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_plain_upstream_keeps_alive_only_when_it_returns_to_a_pool() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let stream = TcpStream::connect(listener.local_addr()?).await?;
    let mut upstream = PlainUpstream {
        stream: stream.into(),
        absolute_form: false,
        proxy_authorization: None,
        returns_to: None,
    };
    assert!(!upstream.keeps_alive());
    let proxy = ParentProxyUrl::new("http://127.0.0.1:3128")?;
    upstream.returns_to = Some(PoolKey::from(&proxy));
    assert!(upstream.keeps_alive());
    Ok(())
}

#[tokio::test]
async fn a_plain_request_without_a_route_dials_the_endpoint_directly() -> TestResult {
    let direct = TcpListener::bind("127.0.0.1:0").await?;
    let direct_port = direct.local_addr()?.port();
    let accepted = tokio::spawn(async move { direct.accept().await.is_ok() });
    let options = NetworkOptions::default();
    let config = tls_config()?;
    let routes = UpstreamRoutes {
        marking: UpstreamMarking::NotNeeded,
        options: &options,
        tls_config: &config,
        parent_pool: &ParentPool::default(),
    };
    let upstream = routes.connect_plain(None, loopback(direct_port)).await?;
    assert!(!upstream.absolute_form);
    assert!(upstream.proxy_authorization.is_none());
    assert!(!upstream.keeps_alive());
    assert!(accepted.await?);
    Ok(())
}

#[tokio::test]
async fn a_name_that_is_not_a_host_is_routed_directly() -> TestResult {
    let direct = TcpListener::bind("127.0.0.1:0").await?;
    let direct_port = direct.local_addr()?.port();
    let accepted = tokio::spawn(async move { direct.accept().await.is_ok() });
    let options = parent_options(Some("http://127.0.0.1:1"), None, "")?;
    let config = tls_config()?;
    let routes = UpstreamRoutes {
        marking: UpstreamMarking::NotNeeded,
        options: &options,
        tls_config: &config,
        parent_pool: &ParentPool::default(),
    };
    routes
        .connect_plain(Some("not a host\r\n"), loopback(direct_port))
        .await?;
    assert!(accepted.await?);
    Ok(())
}

#[tokio::test]
async fn an_https_parent_proxy_is_reached_over_a_secured_connection() -> TestResult {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let issued = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])?;
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        issued.signing_key.serialize_der(),
    ));
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![issued.cert.der().clone()], key)?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(issued.cert.der().clone())?;
    let config = Arc::new(
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_port = listener.local_addr()?.port();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
    let (done, finished) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let secured = acceptor.accept(stream).await?;
        let _ = finished.await;
        drop(secured);
        io::Result::Ok(())
    });
    let options = parent_options(Some(&format!("https://127.0.0.1:{proxy_port}")), None, "")?;
    let routes = UpstreamRoutes {
        marking: UpstreamMarking::NotNeeded,
        options: &options,
        tls_config: &config,
        parent_pool: &ParentPool::default(),
    };
    let upstream = routes.connect_plain(Some("example.com"), endpoint(80)).await?;
    assert!(matches!(upstream.stream, UpstreamStream::Tls(_)));
    assert!(upstream.absolute_form);
    let _ = done.send(());
    server.await??;
    Ok(())
}
