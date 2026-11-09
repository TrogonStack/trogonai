use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

use super::*;

struct FaultThenListener {
    listener: TcpListener,
    attempts: AtomicUsize,
    failed: Notify,
}

impl ReadinessListener for FaultThenListener {
    async fn accept(&self) -> io::Result<(TcpStream, std::net::SocketAddr)> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            self.failed.notify_one();
            Err(io::ErrorKind::ConnectionAborted.into())
        } else {
            self.listener.accept().await
        }
    }
}

#[tokio::test]
async fn an_accept_failure_backs_off_then_serves_a_real_connection_and_honors_shutdown() {
    let readiness = Readiness::new(None, None, Arc::new(Refuse));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = CancellationToken::new();
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _capture = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let task = tokio::spawn(serve_listener(
        readiness.clone(),
        FaultThenListener {
            listener,
            attempts: AtomicUsize::new(0),
            failed: Notify::new(),
        },
        shutdown.clone(),
    ));
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(b"GET /readyz HTTP/1.1\r\n\r\n").await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.starts_with(b"HTTP/1.1 503 Service Unavailable"));
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.message() == Some("HTTP readiness accept failed"))
    );
    shutdown.cancel();
    task.await.unwrap();
    let listener = FaultThenListener {
        listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
        attempts: AtomicUsize::new(0),
        failed: Notify::new(),
    };
    let failed = listener.failed.notified();
    let shutdown = CancellationToken::new();
    let serve = serve_listener(readiness, &listener, shutdown.clone());
    tokio::pin!(serve);
    tokio::select! { () = failed => {}, () = &mut serve => panic!("accept error cannot stop the listener") }
    shutdown.cancel();
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, serve)
        .await
        .unwrap();
}

impl ReadinessListener for &FaultThenListener {
    async fn accept(&self) -> io::Result<(TcpStream, std::net::SocketAddr)> {
        FaultThenListener::accept(self).await
    }
}

struct Refuse;
#[async_trait::async_trait]
impl crate::audit::AuditSink for Refuse {
    async fn record(&self, _: &crate::audit::AuditEvent<'_>) -> Result<(), crate::audit_error::AuditError> {
        Ok(())
    }
}

#[tokio::test]
async fn closing_an_http_request_before_its_head_does_not_write_a_response_or_block_later_probes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let readiness = Readiness::new(None, None, Arc::new(Refuse));
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(readiness.serve(listener, shutdown.clone()));
    let mut closed = TcpStream::connect(address).await.unwrap();
    closed.write_all(b"GET /readyz").await.unwrap();
    closed.shutdown().await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, closed.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.is_empty());
    let mut next = TcpStream::connect(address).await.unwrap();
    next.write_all(b"GET /readyz HTTP/1.1\r\n\r\n").await.unwrap();
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, next.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.starts_with(b"HTTP/1.1 503 Service Unavailable"));
    shutdown.cancel();
    task.await.unwrap();
}
