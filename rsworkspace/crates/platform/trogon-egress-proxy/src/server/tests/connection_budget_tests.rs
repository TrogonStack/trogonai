use std::io::ErrorKind;

use rustls::ClientConnection;

use super::*;
use crate::connection_limit::{ConnectionLimit, ConnectionLimitError};
use crate::constants::TLS_CONTENT_TYPE_HANDSHAKE;

async fn stalled_handshake(address: SocketAddr, pki: &Pki) -> TcpStream {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocols")
        .with_root_certificates(pki.roots())
        .with_no_client_auth();
    let mut client = ClientConnection::new(
        Arc::new(config),
        ServerName::try_from("localhost").expect("server name"),
    )
    .expect("TLS client");
    let mut hello = Vec::new();
    client.write_tls(&mut hello).expect("client hello");
    let mut stream = TcpStream::connect(address).await.expect("TCP connection");
    stream.write_all(&hello).await.expect("send client hello");
    let mut first_byte = [0u8; 1];
    tokio::time::timeout(WAIT, stream.read_exact(&mut first_byte))
        .await
        .expect("the server started this handshake")
        .expect("server hello");
    assert_eq!(first_byte, [TLS_CONTENT_TYPE_HANDSHAKE]);
    stream
}

async fn assert_overload_dropped(address: SocketAddr) {
    let mut overflow = TcpStream::connect(address).await.expect("overflow connection");
    let mut byte = [0u8; 1];
    match tokio::time::timeout(WAIT, overflow.read(&mut byte))
        .await
        .expect("overload must close without waiting for a handshake slot")
    {
        Ok(0) => {}
        Err(error) => assert!(matches!(
            error.kind(),
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
        )),
        result => panic!("an overloaded connection must close before protocol work: {result:?}"),
    }
}

async fn recovered_request(address: SocketAddr, pki: &Pki) {
    let connector = pki.connector(pki.client(&["spiffe://trogon.example/agent/agent-1"]));
    tokio::time::timeout(WAIT, async {
        loop {
            let stream = TcpStream::connect(address).await.expect("recovery connection");
            if let Ok(mut tls) = connector
                .connect(ServerName::try_from("localhost").expect("server name"), stream)
                .await
                && tls.write_all(b"GET / HTTP/1.1\r\n\r\n").await.is_ok()
            {
                let mut reply = [0u8; 1];
                if tls.read(&mut reply).await.is_ok_and(|received| received > 0) {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capacity released by a closed handshake becomes usable");
}

#[tokio::test]
async fn stalled_handshakes_are_bounded_before_spawn_and_capacity_recovers() {
    let pki = Pki::new();
    let limits = Limits {
        max_connections: ConnectionLimit::new(2).expect("connection limit"),
        handshake_timeout: WAIT.saturating_mul(4),
        ..Limits::default()
    };
    let (server, decisions) = server(pki.strict_config(), limits).await;
    let address = server.local_addr().expect("listener address");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let first = stalled_handshake(address, &pki).await;
    let mut second = stalled_handshake(address, &pki).await;

    assert_overload_dropped(address).await;
    assert_eq!(decisions.count(), 0);
    drop(first);
    recovered_request(address, &pki).await;
    assert_eq!(decisions.count(), 1);

    tokio::time::timeout(WAIT, stop(shutdown, task))
        .await
        .expect("shutdown closes held handshake tasks");
    let _ = drain(&mut second).await;
}

#[tokio::test]
async fn authenticated_connections_keep_their_slot_until_the_transport_closes() {
    let pki = Pki::new();
    let limits = Limits {
        max_connections: ConnectionLimit::new(1).expect("connection limit"),
        head_timeout: WAIT.saturating_mul(4),
        ..Limits::default()
    };
    let (server, decisions) = server(pki.strict_config(), limits).await;
    let address = server.local_addr().expect("listener address");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    let connector = pki.connector(pki.client(&["spiffe://trogon.example/agent/agent-1"]));
    let tcp = TcpStream::connect(address).await.expect("TCP connection");
    let authenticated = connector
        .connect(ServerName::try_from("localhost").expect("server name"), tcp)
        .await
        .expect("authenticated TLS connection");

    assert_overload_dropped(address).await;
    assert_eq!(decisions.count(), 0);
    drop(authenticated);
    recovered_request(address, &pki).await;
    assert_eq!(decisions.count(), 1);
    tokio::time::timeout(WAIT, stop(shutdown, task))
        .await
        .expect("bounded tasks shut down");
}

#[test]
fn connection_limits_cannot_disable_the_task_bound() {
    assert_eq!(ConnectionLimit::new(0), Err(ConnectionLimitError::Zero));
    assert_eq!(ConnectionLimit::new(usize::MAX), Err(ConnectionLimitError::TooLarge));
    assert_eq!(ConnectionLimit::new(1).expect("minimum").get(), 1);
    assert_eq!(ConnectionLimit::new(4096).expect("maximum").get(), 4096);
    assert_eq!(ConnectionLimit::new(4097), Err(ConnectionLimitError::TooLarge));
    assert_eq!(ConnectionLimit::default().get(), 128);
}
