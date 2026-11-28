//! Which name the leaf certificate carries, and when a differing server name is refused.

use super::*;

enum ClientTrust {
    TheSandboxCa,
    Anything,
}

/// The status line the client reads for a request to `api.example.com`
/// terminated for a CONNECT to that host, or the handshake error, when the
/// client names `client_name` in its ClientHello (`None` sends no name).
async fn exchange_naming(client_name: Option<&str>, trust: ClientTrust) -> Result<Result<String, String>, TestError> {
    Ok(exchange_naming_with(HostMatch::Required, client_name, trust, true)
        .await?
        .0)
}

async fn exchange_naming_with(
    host_match: HostMatch,
    client_name: Option<&str>,
    trust: ClientTrust,
    with_filter: bool,
) -> Result<(Result<String, String>, usize), TestError> {
    install_default_crypto_provider();
    let hostname = "api.example.com";
    let requests_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&requests_seen);
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
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = tls
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await;
        let _ = tls.shutdown().await;
    });

    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let connector = match trust {
        ClientTrust::TheSandboxCa => client_trusting(&sandbox_ca)?,
        ClientTrust::Anything => TlsConnector::from(Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
                .with_no_client_auth(),
        )),
    };
    let upstream_tls_config = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&upstream_ca.pem)?)
            .with_no_client_auth(),
    );
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    tokio::spawn(async move {
        let Ok((client_tcp, _)) = proxy_listener.accept().await else {
            return;
        };
        let options = NetworkOptions {
            host_match,
            ..NetworkOptions::default()
        };
        let checks = with_filter.then(|| RequestChecks::new(Arc::new(AllowEveryRequest)));
        let _ = terminate_and_forward_checked(
            PrefixedStream::new(client_tcp, vec![]),
            hostname,
            &sandbox_ca,
            vetted(upstream_addr).into(),
            UpstreamMarking::NotNeeded,
            upstream_tls_config,
            None,
            None,
            None,
            patient_deadlines(),
            checks,
            None,
            None,
            Some(&options),
        )
        .await;
    });

    let tcp = TcpStream::connect(proxy_addr).await?;
    let server_name = match client_name {
        Some(name) => rustls::pki_types::ServerName::try_from(name.to_string())?,
        None => rustls::pki_types::ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into()),
    };
    let mut client_tls = match connector.connect(server_name, tcp).await {
        Ok(tls) => tls,
        Err(err) => {
            return Ok((
                Err(err.to_string()),
                requests_seen.load(std::sync::atomic::Ordering::SeqCst),
            ));
        }
    };
    client_tls
        .write_all(
            format!(
                "GET / HTTP/1.1\r\nHost: api.example.com:{}\r\nConnection: close\r\n\r\n",
                upstream_addr.port()
            )
            .as_bytes(),
        )
        .await?;
    let mut response = Vec::new();
    let _ = client_tls.read_to_end(&mut response).await;
    let response = String::from_utf8_lossy(&response).into_owned();
    let status = response.lines().next().unwrap_or_default().to_string();
    Ok((Ok(status), requests_seen.load(std::sync::atomic::Ordering::SeqCst)))
}

#[tokio::test]
async fn the_leaf_is_minted_for_the_target() -> Result<(), TestError> {
    let status = exchange_naming(Some("other.example.com"), ClientTrust::TheSandboxCa).await?;
    assert!(status.is_err(), "{status:?}");
    let status = exchange_naming(Some("api.example.com"), ClientTrust::TheSandboxCa).await?;
    assert_eq!(status.as_deref(), Ok("HTTP/1.1 200 OK"));
    Ok(())
}

#[tokio::test]
async fn an_unreadable_name_is_answered_421() -> Result<(), TestError> {
    let status = exchange_naming(Some("under_score.example.com"), ClientTrust::Anything).await?;
    assert_eq!(status.as_deref(), Ok("HTTP/1.1 421 Misdirected Request"));
    Ok(())
}

#[tokio::test]
async fn a_name_other_than_the_target_is_answered_421() -> Result<(), TestError> {
    let status = exchange_naming(Some("other.example.com"), ClientTrust::Anything).await?;
    assert_eq!(status.as_deref(), Ok("HTTP/1.1 421 Misdirected Request"));
    Ok(())
}

#[tokio::test]
async fn host_matching_runs_with_no_request_filter_secrets_or_forward_rule() -> Result<(), TestError> {
    for name in ["other.example.com", "under_score.example.com"] {
        let (status, _) = exchange_naming_with(HostMatch::Required, Some(name), ClientTrust::Anything, false).await?;
        assert_eq!(status.as_deref(), Ok("HTTP/1.1 421 Misdirected Request"), "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn with_matching_off_the_leaf_is_minted_for_the_name_the_client_asked_for() -> Result<(), TestError> {
    let (status, requests_seen) = exchange_naming_with(
        HostMatch::NotRequired,
        Some("other.example.com"),
        ClientTrust::TheSandboxCa,
        true,
    )
    .await?;
    assert_eq!(status.as_deref(), Ok("HTTP/1.1 200 OK"));
    assert_eq!(requests_seen, 1);
    Ok(())
}

#[tokio::test]
async fn with_matching_off_an_unreadable_name_is_not_refused() -> Result<(), TestError> {
    let (status, requests_seen) = exchange_naming_with(
        HostMatch::NotRequired,
        Some("under_score.example.com"),
        ClientTrust::Anything,
        true,
    )
    .await?;
    assert_eq!(status.as_deref(), Ok("HTTP/1.1 200 OK"));
    assert_eq!(requests_seen, 1);
    Ok(())
}

#[tokio::test]
async fn with_matching_off_a_hello_without_a_name_gets_the_leaf_for_the_target() -> Result<(), TestError> {
    let (status, _) = exchange_naming_with(HostMatch::NotRequired, None, ClientTrust::Anything, true).await?;
    assert_eq!(status.as_deref(), Ok("HTTP/1.1 200 OK"));
    Ok(())
}

#[tokio::test]
async fn a_hello_without_a_name_gets_the_leaf_for_the_target() -> Result<(), TestError> {
    let (status, requests_seen) = exchange_naming_with(HostMatch::Required, None, ClientTrust::Anything, true).await?;
    assert_eq!(status.as_deref(), Ok("HTTP/1.1 200 OK"));
    assert_eq!(requests_seen, 1);
    Ok(())
}
