use mcp_postgres::{Error, SpiceDb};
use tokio::{io::AsyncReadExt, net::TcpListener};

#[tokio::test]
async fn https_authorization_endpoint_requires_tls_and_never_sends_a_plaintext_token()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("https://{}", listener.local_addr()?);
    let received = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut bytes = [0; 1024];
        let count = socket.read(&mut bytes).await?;
        Ok::<_, std::io::Error>(bytes[..count].to_vec())
    });
    assert!(matches!(
        SpiceDb::connect(endpoint, "private-transport-token".into()).await,
        Err(Error::AuthorizationConnection(_))
    ));
    let bytes = received.await??;
    assert_eq!(bytes.first(), Some(&22), "the first record must be a TLS handshake");
    assert!(!String::from_utf8_lossy(&bytes).contains("private-transport-token"));
    Ok(())
}
