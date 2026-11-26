use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use trogon_sandbox_observability::{
    BROKER, ObservabilityArgs, ObservabilityError, default_registry, gather_text, metrics_addr, serve_metrics,
    spawn_metrics_server,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn free_addr() -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?)
}

async fn fetch(addr: SocketAddr) -> Result<reqwest::Response, Box<dyn std::error::Error>> {
    let url = format!("http://{addr}/metrics");
    for _ in 0..100 {
        if let Ok(response) = reqwest::get(&url).await {
            return Ok(response);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("metrics endpoint never became reachable".into())
}

#[test]
fn metrics_addr_offsets_by_1000() -> TestResult {
    assert_eq!(metrics_addr(8090), "127.0.0.1:9090".parse()?);
    assert_eq!(metrics_addr(8443), "127.0.0.1:9443".parse()?);
    assert_eq!(metrics_addr(BROKER), "127.0.0.1:9090".parse()?);
    Ok(())
}

#[test]
fn metrics_addr_saturates_at_max_port() {
    assert_eq!(metrics_addr(u16::MAX).port(), u16::MAX);
}

#[test]
fn explicit_metrics_addr_wins() -> TestResult {
    let args = ObservabilityArgs::try_parse_from(["bin", "--metrics-addr", "127.0.0.1:7777"])?;
    assert_eq!(args.resolve_metrics_addr(8090), "127.0.0.1:7777".parse()?);
    Ok(())
}

#[test]
fn missing_metrics_addr_uses_default() -> TestResult {
    let args = ObservabilityArgs::try_parse_from(["bin"])?;
    assert_eq!(args.resolve_metrics_addr(8090), metrics_addr(8090));
    Ok(())
}

#[test]
fn invalid_metrics_addr_is_rejected_at_parse_time() {
    assert!(ObservabilityArgs::try_parse_from(["bin", "--metrics-addr", "nope"]).is_err());
}

#[test]
fn default_registry_exposes_build_info() -> TestResult {
    let text = gather_text(&default_registry("worker")?)?;
    assert!(text.contains("sandbox_build_info 1"));
    Ok(())
}

#[tokio::test]
async fn serve_metrics_returns_registry_text() -> TestResult {
    let addr = free_addr()?;
    let registry = default_registry("worker")?;
    let server = tokio::spawn(serve_metrics(addr, registry));
    let response = fetch(addr).await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").map(|v| v.as_bytes()),
        Some("text/plain; charset=utf-8".as_bytes())
    );
    assert!(response.text().await?.contains("sandbox_build_info 1"));
    server.abort();
    Ok(())
}

#[tokio::test]
async fn spawned_server_serves_metrics() -> TestResult {
    let addr = free_addr()?;
    spawn_metrics_server(addr, default_registry("worker")?);
    assert_eq!(fetch(addr).await?.status(), reqwest::StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn serve_metrics_reports_bind_failure() -> TestResult {
    let held = std::net::TcpListener::bind("127.0.0.1:0")?;
    let addr = held.local_addr()?;
    let error = serve_metrics(addr, default_registry("worker")?).await.err();
    assert!(matches!(error, Some(ObservabilityError::Listener { addr: failed, .. }) if failed == addr));
    Ok(())
}

#[tokio::test]
async fn spawned_server_survives_bind_failure() -> TestResult {
    let held = std::net::TcpListener::bind("127.0.0.1:0")?;
    spawn_metrics_server(held.local_addr()?, default_registry("worker")?);
    tokio::task::yield_now().await;
    Ok(())
}
