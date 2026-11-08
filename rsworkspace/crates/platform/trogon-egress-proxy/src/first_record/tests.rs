use std::sync::Arc;

use tokio::io::AsyncWriteExt;

use super::*;

/// The first flight of a real TLS client, so the parser is checked against
/// what a TLS stack actually emits.
fn client_hello(name: &str) -> Vec<u8> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let server_name = rustls_pki_types::ServerName::try_from(name.to_owned()).expect("name");
    let mut connection = rustls::ClientConnection::new(Arc::new(config), server_name).expect("client");
    let mut out = Vec::new();
    connection.write_tls(&mut out).expect("hello");
    out
}

const LONG: Duration = Duration::from_secs(5);

#[tokio::test]
async fn returns_the_name_and_every_byte_read() {
    let hello = client_hello("example.org");
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    client.write_all(&hello).await.expect("write");
    let (host, bytes) = read_first_record(&mut server, Vec::new(), LONG).await.expect("record");
    assert_eq!(host.as_str(), "example.org");
    assert_eq!(bytes, hello);
}

#[tokio::test]
async fn bytes_already_read_come_first_and_the_rest_is_awaited() {
    let hello = client_hello("example.org");
    let (head, tail) = hello.split_at(7);
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    let tail = tail.to_vec();
    let writer = tokio::spawn(async move {
        for piece in tail.chunks(100) {
            client.write_all(piece).await.expect("write");
            tokio::task::yield_now().await;
        }
        client
    });
    let (host, bytes) = read_first_record(&mut server, head.to_vec(), LONG)
        .await
        .expect("record");
    assert_eq!(host.as_str(), "example.org");
    assert_eq!(bytes, hello);
    drop(writer.await.expect("writer"));
}

#[tokio::test]
async fn bytes_after_the_record_are_never_dropped() {
    let hello = client_hello("example.org");
    let mut wire = hello.clone();
    wire.extend_from_slice(b"early data");
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    client.write_all(&wire).await.expect("write");
    let (_, bytes) = read_first_record(&mut server, Vec::new(), LONG).await.expect("record");
    assert!(bytes.len() >= hello.len());
    assert!(wire.starts_with(&bytes));
}

#[tokio::test]
async fn plaintext_is_refused_without_waiting_for_more() {
    let (mut client, mut server) = tokio::io::duplex(1024);
    client.write_all(b"GET / HTTP/1.1\r\n").await.expect("write");
    let error = read_first_record(&mut server, Vec::new(), LONG)
        .await
        .expect_err("refused");
    assert!(matches!(
        error,
        FirstRecordError::ClientHello(ClientHelloError::NotTlsHandshake)
    ));
}

#[tokio::test]
async fn a_close_mid_record_is_reported() {
    let hello = client_hello("example.org");
    let (mut client, mut server) = tokio::io::duplex(1024);
    client.write_all(&hello[..20]).await.expect("write");
    drop(client);
    let error = read_first_record(&mut server, Vec::new(), LONG)
        .await
        .expect_err("closed");
    assert!(matches!(error, FirstRecordError::Closed));
}

#[tokio::test]
async fn silence_times_out() {
    let (_client, mut server) = tokio::io::duplex(1024);
    let error = read_first_record(&mut server, Vec::new(), Duration::from_millis(50))
        .await
        .expect_err("timeout");
    assert!(matches!(error, FirstRecordError::TimedOut));
}
