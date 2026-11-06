use super::*;
use crate::ReadinessStatus;

#[derive(Default)]
struct ObservedDrain {
    entered: tokio::sync::Notify,
    stopped: tokio::sync::Notify,
}

#[async_trait]
impl AuditSink for ObservedDrain {
    async fn record(&self, _: &AuditEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }

    async fn run(&self, shutdown: CancellationToken) {
        self.entered.notify_one();
        shutdown.cancelled().await;
        self.stopped.notify_one();
    }
}

#[tokio::test]
async fn aborting_the_public_server_future_cancels_its_background_audit_drain() {
    let audit = Arc::new(ObservedDrain::default());
    let server = audited_server(Pki::new().strict_config(), Limits::default(), audit.clone()).await;
    let task = tokio::spawn(server.run(CancellationToken::new()));
    tokio::time::timeout(WAIT, audit.entered.notified())
        .await
        .expect("audit drain entered");
    task.abort();
    assert!(task.await.expect_err("aborted server").is_cancelled());
    let stopped = tokio::time::timeout(WAIT, audit.stopped.notified()).await;
    assert!(
        stopped.is_ok(),
        "dropping Server::run must signal its live background audit drain"
    );
}

async fn http(address: SocketAddr, request: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(address).await.expect("HTTP readiness port");
    stream.write_all(request).await.expect("HTTP request");
    drain(&mut stream).await
}

#[tokio::test]
async fn the_public_readiness_listener_bounds_and_refuses_untrusted_http_heads() {
    let (server, _) = server(Pki::new().strict_config(), Limits::default()).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("readiness listener");
    let address = listener.local_addr().expect("readiness address");
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.readiness().serve(listener, shutdown.clone()));
    let cases: &[(&[u8], &[u8])] = &[
        (
            b"GET /readyz HTTP/1.1\r\nHost: localhost\r\n\r\n",
            b"HTTP/1.1 503 Service Unavailable\r\n",
        ),
        (
            b"GET /private HTTP/1.1\r\nHost: localhost\r\n\r\n",
            b"HTTP/1.1 404 Not Found\r\n",
        ),
        (
            b"POST /readyz HTTP/1.1\r\nHost: localhost\r\n\r\n",
            b"HTTP/1.1 404 Not Found\r\n",
        ),
        (
            b"GET /readyz HTTP/1.1\r\ninvalid-header\r\n\r\n",
            b"HTTP/1.1 400 Bad Request\r\n",
        ),
        (
            b"GET /readyz HTTP/1.1\r\nContent-Length: 1\r\n\r\n",
            b"HTTP/1.1 400 Bad Request\r\n",
        ),
        (
            b"GET /readyz HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"HTTP/1.1 400 Bad Request\r\n",
        ),
        (
            b"GET /readyz HTTP/1.1\r\nHost: \xff\r\n\r\n",
            b"HTTP/1.1 400 Bad Request\r\n",
        ),
        (
            b"GET /readyz HTTP/1.1\r\nHost: \x00\r\n\r\n",
            b"HTTP/1.1 400 Bad Request\r\n",
        ),
        (b"GET /readyz HTTP/1.1\r\n", b"HTTP/1.1 408 Request Timeout\r\n"),
    ];
    for (request, expected) in cases {
        let reply = http(address, request).await;
        assert!(reply.starts_with(expected), "unexpected readiness response {reply:?}");
    }
    let pipelined = http(address, b"GET /readyz HTTP/1.1\r\n\r\npipeline").await;
    assert!(
        pipelined.starts_with(b"HTTP/1.1 400 Bad Request\r\n")
            || pipelined.starts_with(b"HTTP/1.1 503 Service Unavailable\r\n")
    );
    assert_eq!(
        pipelined
            .windows(b"HTTP/1.1".len())
            .filter(|bytes| *bytes == b"HTTP/1.1")
            .count(),
        1
    );
    let oversized = vec![b'a'; 1024];
    assert!(
        http(address, &oversized)
            .await
            .starts_with(b"HTTP/1.1 431 Request Header Fields Too Large\r\n")
    );
    shutdown.cancel();
    task.await.expect("bounded HTTP readiness shutdown");
}

#[tokio::test]
async fn a_bound_port_and_a_compatibility_profile_cannot_claim_enterprise_readiness() {
    let (server, _) = server(Pki::new().strict_config(), Limits::default()).await;
    let readiness = server.readiness();
    assert_eq!(readiness.probe().await, ReadinessStatus::Starting);
    let transport = TcpStream::connect(server.local_addr().expect("bound address"))
        .await
        .expect("bound port");
    drop(transport);
    assert_eq!(readiness.probe().await, ReadinessStatus::Starting);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown));
    tokio::time::timeout(WAIT, async {
        while readiness.probe().await == ReadinessStatus::Starting {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the server runs");
    assert_eq!(readiness.probe().await, ReadinessStatus::UnsupportedProfile);
    task.abort();
    assert!(task.await.expect_err("aborted runtime").is_cancelled());
    assert_eq!(readiness.probe().await, ReadinessStatus::Stopped);
}
