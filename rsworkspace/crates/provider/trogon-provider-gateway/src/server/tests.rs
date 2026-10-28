use super::{ServerError, bind, serve};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, oneshot};
use tokio::time::timeout;
use tower::ServiceExt;
use tracing_subscriber::prelude::*;
use trogon_provider_gateway::{Health, router};

const DEADLINE: Duration = Duration::from_secs(5);

async fn request(address: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

fn assert_ok_response(response: &str) {
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "unexpected HTTP status; expected 200 OK"
    );
}

#[tokio::test]
async fn serves_real_tcp_health_requests_and_stops_on_shutdown() {
    timeout(DEADLINE, async {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
        let address = listener.local_addr().unwrap();
        assert_ne!(address.port(), 0);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let health = Health::default();
        let task = tokio::spawn(serve(listener, router(health.clone()), health, async move {
            shutdown_rx.await.unwrap();
        }));

        for path in ["/-/liveness", "/-/readiness"] {
            let response = request(address, path).await;
            assert_ok_response(&response);
        }

        shutdown_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
        assert!(TcpStream::connect(address).await.is_err());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn graceful_shutdown_drains_an_in_flight_request() {
    timeout(DEADLINE, async {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
        let address = listener.local_addr().unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let app = Router::new().route(
            "/blocked",
            get({
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                move || {
                    let entered = Arc::clone(&entered);
                    let release = Arc::clone(&release);
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        "drained"
                    }
                }
            }),
        );
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (observed_tx, observed_rx) = oneshot::channel();
        let health = Health::default();
        let probes = router(health.clone());
        let mut server = tokio::spawn(serve(listener, app.merge(probes.clone()), health, async move {
            shutdown_rx.await.unwrap();
            observed_tx.send(()).unwrap();
        }));
        let client = tokio::spawn(request(address, "/blocked"));
        entered.notified().await;
        shutdown_tx.send(()).unwrap();
        observed_rx.await.unwrap();

        for (path, expected) in [
            ("/-/readiness", StatusCode::SERVICE_UNAVAILABLE),
            ("/-/liveness", StatusCode::OK),
        ] {
            let response = probes
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path} during draining");
        }

        tokio::select! {
            biased;
            result = &mut server => panic!("server stopped before the request completed: {result:?}"),
            () = std::future::ready(()) => {}
        }
        release.notify_one();

        let response = client.await.unwrap();
        assert_ok_response(&response);
        assert!(
            response.ends_with("drained"),
            "in-flight request did not finish with the expected body"
        );
        server.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn occupied_bind_address_surfaces_the_original_error() {
    timeout(DEADLINE, async {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
        let address = listener.local_addr().unwrap();

        assert!(matches!(
            bind(address).await,
            Err(ServerError::Bind { address: failed_address, source })
                if failed_address == address && source.kind() == std::io::ErrorKind::AddrInUse
        ));
    })
    .await
    .unwrap();
}

struct ServerSpans(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ServerSpans {
    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        _id: &tracing::Id,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if attributes.metadata().name() == "http.server.request" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[tokio::test]
async fn operational_requests_create_no_server_spans() {
    let spans = Arc::new(AtomicUsize::new(0));
    tracing_subscriber::registry()
        .with(ServerSpans(Arc::clone(&spans)))
        .init();
    timeout(DEADLINE, async {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let health = Health::default();
        let server = tokio::spawn(serve(listener, router(health.clone()), health, async move {
            shutdown_rx.await.unwrap();
        }));
        for path in ["/-/liveness", "/-/readiness"] {
            assert!(request(address, path).await.starts_with("HTTP/1.1 200 OK"));
        }
        shutdown_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
    })
    .await
    .unwrap();
    assert_eq!(spans.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn serves_http2_on_the_same_listener_as_http1() {
    timeout(DEADLINE, async {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let health = Health::default();
        let server = tokio::spawn(serve(listener, router(health.clone()), health, async move {
            shutdown_rx.await.unwrap();
        }));
        assert!(request(address, "/-/readiness").await.starts_with("HTTP/1.1 200 OK"));
        let stream = TcpStream::connect(address).await.unwrap();
        let (mut client, connection) = h2::client::handshake(stream).await.unwrap();
        let connection = tokio::spawn(connection);
        for path in ["/-/liveness", "/-/readiness"] {
            let request = Request::builder()
                .uri(format!("http://{address}{path}"))
                .body(())
                .unwrap();
            let (response, _) = client.send_request(request, true).unwrap();
            assert_eq!(response.await.unwrap().status(), StatusCode::OK);
        }
        drop(client);
        connection.abort();
        shutdown_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn peer_content_cannot_forge_response_assertion_diagnostics() {
    timeout(DEADLINE, async {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(&mut socket);
            let mut headers = String::new();
            while tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut headers)
                .await
                .unwrap()
                != 0
            {
                if headers.ends_with("\r\n\r\n") {
                    break;
                }
            }
            drop(reader);
            socket
                .write_all(b"HTTP/1.1 503 Unavailable\r\nConnection: close\r\n\r\nFORGED_LOG_ENTRY\r\n")
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        });
        let response = request(address, "/-/liveness").await;
        peer.await.unwrap();
        let panic = std::panic::catch_unwind(|| assert_ok_response(&response)).unwrap_err();
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(
            !message.contains("FORGED_LOG_ENTRY"),
            "peer content leaked into assertion output"
        );
    })
    .await
    .unwrap();
}
