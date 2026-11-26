use tracing::Level;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::*;

pub(super) fn client_params(uri: &str) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.is_ca = IsCa::ExplicitNoCa;
    params.subject_alt_names = vec![SanType::URI(Ia5String::try_from(uri).expect("ia5"))];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    one_hour_validity(&mut params);
    params
}

#[tokio::test]
async fn a_refused_agent_certificate_is_logged_with_a_reason_and_the_verified_peer_id() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let pki = Pki::new();
    let (server, _) = server(pki.lax_config(), Limits::default()).await;
    let address = server.local_addr().expect("addr");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));

    let foreign = pki.issue(client_params("spiffe://other.example/agent/agent-1"));
    let tcp = TcpStream::connect(address).await.expect("connect");
    let mut stream = pki
        .connector(foreign)
        .connect(ServerName::try_from("localhost").expect("name"), tcp)
        .await
        .expect("handshake");
    stream.write_all(b"CONNECT a:443 HTTP/1.1\r\n\r\n").await.ok();
    assert!(drain(&mut stream).await.is_empty());
    stop(shutdown, task).await;

    let warnings = captured.at(Level::WARN);
    let refusal = warnings
        .iter()
        .find(|event| event.field("reason") == Some("client_certificate_names_no_agent"))
        .unwrap_or_else(|| panic!("{warnings:?}"));
    assert_eq!(
        refusal.field("peer.spiffe_id"),
        Some("spiffe://other.example/agent/agent-1")
    );
    assert!(refusal.field("peer").is_some());
}

#[tokio::test]
async fn a_refused_handshake_is_logged_with_a_reason_and_no_certificate_material() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let pki = Pki::new();
    let (server, _) = server(pki.strict_config(), Limits::default()).await;
    let address = server.local_addr().expect("addr");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));

    let mut params = client_params("spiffe://trogon.example/agent/agent-1");
    params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    let tcp = TcpStream::connect(address).await.expect("connect");
    let mut stream = pki
        .connector(pki.issue(params))
        .connect(ServerName::try_from("localhost").expect("name"), tcp)
        .await
        .expect("the client finishes before the server verifies");
    stream.write_all(b"CONNECT a:443 HTTP/1.1\r\n\r\n").await.ok();
    assert!(drain(&mut stream).await.is_empty());
    stop(shutdown, task).await;

    let warnings = captured.at(Level::WARN);
    let refusal = warnings
        .iter()
        .find(|event| event.field("reason") == Some("tls_handshake_refused"))
        .unwrap_or_else(|| panic!("{warnings:?}"));
    assert!(
        refusal
            .field("error")
            .is_some_and(|error| error.contains("SVID key profile"))
    );
    assert!(!format!("{warnings:?}").contains("BEGIN"));
}

#[tokio::test]
async fn an_over_long_client_certificate_is_logged_with_its_own_reason() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let pki = Pki::new();
    let (server, _) = server(pki.strict_config(), Limits::default()).await;
    let address = server.local_addr().expect("addr");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));

    let mut params = client_params("spiffe://trogon.example/agent/agent-1");
    params.not_after = (std::time::SystemTime::now() + Duration::from_secs(2 * 3600)).into();
    let tcp = TcpStream::connect(address).await.expect("connect");
    let mut stream = pki
        .connector(pki.issue(params))
        .connect(ServerName::try_from("localhost").expect("name"), tcp)
        .await
        .expect("the client finishes before the server verifies");
    stream.write_all(b"CONNECT a:443 HTTP/1.1\r\n\r\n").await.ok();
    assert!(drain(&mut stream).await.is_empty());
    stop(shutdown, task).await;

    let warnings = captured.at(Level::WARN);
    assert!(
        warnings
            .iter()
            .any(|event| event.field("reason") == Some("client_certificate_lifetime_exceeded")),
        "{warnings:?}"
    );
    assert!(!format!("{warnings:?}").contains("BEGIN"));
}

#[tokio::test]
async fn a_stalled_handshake_is_logged_as_a_timeout() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::INFO);
    let limits = Limits {
        handshake_timeout: Duration::from_millis(50),
        ..Limits::default()
    };
    let (server, _) = server(Pki::new().strict_config(), limits).await;
    let address = server.local_addr().expect("addr");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let mut client = TcpStream::connect(address).await.expect("connect");
    assert!(drain(&mut client).await.is_empty());
    stop(shutdown, task).await;

    assert!(
        captured
            .at(Level::WARN)
            .iter()
            .any(|event| event.field("reason") == Some("tls_handshake_timed_out"))
    );
}
