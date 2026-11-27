use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use trogon_sandbox_observability::{
    BROKER, Health, ObservabilityArgs, ObservabilityError, default_registry, gather_text, metrics_addr, serve_metrics,
    serve_metrics_on, spawn_metrics_server,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
async fn serve_metrics_returns_registry_text_until_shut_down() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move { drop(stopped.await) };
    let server = tokio::spawn(serve_metrics_on(
        listener,
        default_registry("worker")?,
        Health::new(),
        shutdown,
    ));
    let response = fetch(addr).await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").map(|v| v.as_bytes()),
        Some("text/plain; charset=utf-8".as_bytes())
    );
    assert!(response.text().await?.contains("sandbox_build_info 1"));
    stop.send(()).map_err(|()| "server stopped before shutdown")?;
    tokio::time::timeout(Duration::from_secs(5), server).await???;
    Ok(())
}

#[tokio::test]
async fn serve_metrics_keeps_serving_on_the_address_it_binds() -> TestResult {
    let server = tokio::spawn(serve_metrics(
        "127.0.0.1:0".parse()?,
        default_registry("worker")?,
        Health::new(),
    ));
    tokio::task::yield_now().await;
    assert!(!server.is_finished());
    server.abort();
    Ok(())
}

#[tokio::test]
async fn serve_metrics_reports_bind_failure() -> TestResult {
    let held = std::net::TcpListener::bind("127.0.0.1:0")?;
    let addr = held.local_addr()?;
    let error = serve_metrics(addr, default_registry("worker")?, Health::new())
        .await
        .err();
    assert!(matches!(error, Some(ObservabilityError::Listener { addr: failed, .. }) if failed == addr));
    Ok(())
}

#[tokio::test]
async fn spawned_server_survives_bind_failure() -> TestResult {
    let held = std::net::TcpListener::bind("127.0.0.1:0")?;
    spawn_metrics_server(held.local_addr()?, default_registry("worker")?, Health::new());
    tokio::task::yield_now().await;
    Ok(())
}
