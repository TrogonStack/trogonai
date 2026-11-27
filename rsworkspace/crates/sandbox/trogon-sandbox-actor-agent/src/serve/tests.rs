use std::error::Error;

use super::*;

#[tokio::test]
async fn a_bound_listener_is_served_until_cancelled() -> Result<(), Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let cancel = CancellationToken::new();
    cancel.cancel();
    serve_bound(Ok(listener), Router::new(), cancel).await?;
    Ok(())
}

#[tokio::test]
async fn a_bind_failure_is_the_result() {
    let failed: std::io::Result<tokio::net::TcpListener> = Err(std::io::ErrorKind::AddrInUse.into());
    let outcome = serve_bound(failed, Router::new(), CancellationToken::new()).await;
    assert_eq!(
        outcome.err().map(|error| error.kind()),
        Some(std::io::ErrorKind::AddrInUse)
    );
}
