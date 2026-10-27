use super::{ServerError, bind, serve};
use axum::{Router, routing::get};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, oneshot};
use tokio::time::timeout;

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

#[tokio::test]
async fn serves_real_tcp_health_requests_and_stops_on_shutdown() {
    timeout(DEADLINE, async {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await.unwrap();
        let address = listener.local_addr().unwrap();
        assert_ne!(address.port(), 0);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(serve(listener, trogon_provider_gateway::router(), async move {
            shutdown_rx.await.unwrap();
        }));

        for path in ["/", "/healthz"] {
            let response = request(address, path).await;
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
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
        let mut server = tokio::spawn(serve(listener, app, async move {
            shutdown_rx.await.unwrap();
            observed_tx.send(()).unwrap();
        }));
        let client = tokio::spawn(request(address, "/blocked"));
        entered.notified().await;
        shutdown_tx.send(()).unwrap();
        observed_rx.await.unwrap();

        tokio::select! {
            biased;
            result = &mut server => panic!("server stopped before the request completed: {result:?}"),
            () = std::future::ready(()) => {}
        }
        release.notify_one();

        let response = client.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.ends_with("drained"), "{response}");
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
