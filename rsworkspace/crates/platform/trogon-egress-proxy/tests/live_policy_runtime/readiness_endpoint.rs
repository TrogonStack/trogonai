use super::*;

async fn http(address: SocketAddr, request: &[u8]) -> TestResult<Vec<u8>> {
    let mut stream = TcpStream::connect(address).await?;
    stream.write_all(request).await?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    Ok(reply)
}

#[tokio::test]
async fn the_http_readiness_endpoint_requires_a_current_durable_audit_backend() -> TestResult<()> {
    let harness = WorkloadHarness::start("").await?;
    status_eventually(&harness.readiness, ReadinessStatus::Ready).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(harness.readiness.clone().serve(listener, shutdown.clone()));
    let first = tokio::time::timeout(WAIT, http(address, b"GET /readyz HTTP/1.1\r\nHost: localhost\r\n\r\n")).await;
    if first.is_err() {
        shutdown.cancel();
        task.await?;
        harness.stop().await?;
        panic!("the public HTTP readiness listener must answer within the bound");
    }
    let ready = first??;
    assert!(ready.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(ready.ends_with(b"\r\n\r\nready\n"));
    harness.context.delete_stream(trogon_audit::constants::STREAM_AUDIT).await?;
    let unavailable = tokio::time::timeout(WAIT,
        http(address, b"GET /readyz HTTP/1.1\r\nHost: localhost\r\n\r\n"),
    ).await??;
    assert!(unavailable.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n"));
    assert!(unavailable.ends_with(b"\r\n\r\nunavailable\n"));
    assert!(!String::from_utf8_lossy(&unavailable).contains(LOCAL_WORKLOAD));
    let invalid = tokio::time::timeout(WAIT, http(address, b"GET /private HTTP/1.1\r\nHost: localhost\r\n\r\n")).await??;
    assert!(invalid.starts_with(b"HTTP/1.1 404 Not Found\r\n"));
    shutdown.cancel();
    task.await?;
    harness.stop().await
}
