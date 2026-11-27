use std::net::SocketAddr;
use std::time::Duration;

use reqwest::StatusCode;
use trogon_sandbox_observability::{Health, default_registry, serve_metrics_on};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn status(addr: SocketAddr, path: &str) -> Result<StatusCode, Box<dyn std::error::Error>> {
    let url = format!("http://{addr}{path}");
    for _ in 0..100 {
        if let Ok(response) = reqwest::get(&url).await {
            return Ok(response.status());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("listener never became reachable".into())
}

#[tokio::test]
async fn private_listener_serves_the_operational_paths_beside_metrics() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let health = Health::new();
    tokio::spawn(serve_metrics_on(
        listener,
        default_registry("worker")?,
        health.clone(),
        std::future::pending(),
    ));

    assert_eq!(status(addr, "/-/readiness").await?, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(status(addr, "/-/liveness").await?, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(status(addr, "/metrics").await?, StatusCode::OK);

    health.mark_started();
    assert_eq!(status(addr, "/-/readiness").await?, StatusCode::OK);
    assert_eq!(status(addr, "/-/liveness").await?, StatusCode::OK);

    health.begin_shutdown();
    assert_eq!(status(addr, "/-/readiness").await?, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(status(addr, "/-/liveness").await?, StatusCode::OK);
    Ok(())
}
