use super::*;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::tls_ca::install_default_crypto_provider;
use crate::tls_intercept::tests::{build_test_upstream_ca_and_leaf, root_store_from_pem};

#[tokio::test]
async fn a_tcp_upstream_reads_writes_flushes_and_shuts_down() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (mut peer, _) = listener.accept().await?;
        let mut got = Vec::new();
        peer.read_to_end(&mut got).await?;
        peer.write_all(b"pong").await?;
        Ok::<_, std::io::Error>(got)
    });
    let mut stream = UpstreamStream::from(TcpStream::connect(address).await?);
    stream.write_all(b"ping").await?;
    stream.flush().await?;
    stream.shutdown().await?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    assert_eq!(reply, b"pong");
    assert_eq!(server.await??, b"ping");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_unix_upstream_reads_writes_flushes_and_shuts_down() -> Result<(), Box<dyn std::error::Error>> {
    let (near, mut far) = tokio::net::UnixStream::pair()?;
    let mut stream = UpstreamStream::Unix(near);
    stream.write_all(b"ping").await?;
    stream.flush().await?;
    stream.shutdown().await?;
    let mut got = Vec::new();
    far.read_to_end(&mut got).await?;
    assert_eq!(got, b"ping");
    far.write_all(b"pong").await?;
    drop(far);
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    assert_eq!(reply, b"pong");
    Ok(())
}

#[tokio::test]
async fn a_tls_upstream_reads_writes_flushes_and_shuts_down() -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let ca = build_test_upstream_ca_and_leaf("upstream.example.com")?;
    let acceptor = TlsAcceptor::from(Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![ca.cert.clone()], ca.key.clone_key())?,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await?;
        let mut peer = acceptor.accept(tcp).await?;
        let mut got = Vec::new();
        peer.read_to_end(&mut got).await?;
        peer.write_all(b"pong").await?;
        peer.shutdown().await?;
        Ok::<_, std::io::Error>(got)
    });
    let connector = tokio_rustls::TlsConnector::from(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store_from_pem(&ca.pem)?)
            .with_no_client_auth(),
    ));
    let name = rustls::pki_types::ServerName::try_from("upstream.example.com")?;
    let secured = connector.connect(name, TcpStream::connect(address).await?).await?;
    let mut stream = UpstreamStream::Tls(Box::new(secured));
    stream.write_all(b"ping").await?;
    stream.flush().await?;
    stream.shutdown().await?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    assert_eq!(reply, b"pong");
    assert_eq!(server.await??, b"ping");
    Ok(())
}
