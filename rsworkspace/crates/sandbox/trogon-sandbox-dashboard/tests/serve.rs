use std::net::SocketAddr;

use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use trogon_sandbox_dashboard::{
    DashboardArgs, DashboardConfig, DashboardError, WsGatewayUrl, WsGatewayUrlError, router, run,
    serve_app_with_shutdown, serve_with_shutdown,
};
use trogon_sandbox_observability::Health;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Running {
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), DashboardError>>,
}

async fn start() -> Result<Running, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (stop, stopped) = oneshot::channel::<()>();
    let config = DashboardConfig {
        ws_gateway_url: WsGatewayUrl::parse("ws://127.0.0.1:8443/v1/ws")?,
    };
    let task = tokio::spawn(serve_with_shutdown(listener, config, async move {
        let _ = stopped.await;
        Ok(())
    }));
    Ok(Running { addr, stop, task })
}

async fn finish(running: Running) -> TestResult {
    let _ = running.stop.send(());
    running.task.await??;
    Ok(())
}

#[tokio::test]
async fn root_returns_html_bundle() -> TestResult {
    let running = start().await?;
    let response = reqwest::get(format!("http://{}/", running.addr)).await?;
    assert_eq!(response.headers()["content-type"], "text/html; charset=utf-8");
    let body = response.text().await?;
    assert!(body.contains("<title>Sandbox Dashboard</title>"));
    for panel in ["panel-leases", "panel-snapshots", "panel-workers", "panel-tags"] {
        assert!(body.contains(&format!("id=\"{panel}\"")));
    }
    assert!(body.contains("/app.js"));
    finish(running).await
}

#[tokio::test]
async fn serves_each_embedded_asset_with_its_content_type() -> TestResult {
    let running = start().await?;
    for (path, content_type) in [
        ("/index.html", "text/html; charset=utf-8"),
        ("/app.js", "text/javascript; charset=utf-8"),
        ("/style.css", "text/css; charset=utf-8"),
    ] {
        let response = reqwest::get(format!("http://{}{path}", running.addr)).await?;
        assert!(response.status().is_success());
        assert_eq!(response.headers()["content-type"], content_type);
        assert!(!response.text().await?.is_empty());
    }
    finish(running).await
}

#[tokio::test]
async fn config_json_exposes_the_gateway_url() -> TestResult {
    let running = start().await?;
    let value: serde_json::Value = reqwest::get(format!("http://{}/config.json", running.addr))
        .await?
        .json()
        .await?;
    assert_eq!(value["ws_gateway_url"], "ws://127.0.0.1:8443/v1/ws");
    finish(running).await
}

#[tokio::test]
async fn unknown_path_is_not_found() -> TestResult {
    let running = start().await?;
    let response = reqwest::get(format!("http://{}/missing", running.addr)).await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    finish(running).await
}

#[tokio::test]
async fn shutdown_signal_failure_is_reported() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let config = DashboardConfig {
        ws_gateway_url: WsGatewayUrl::parse("wss://example.test/ws")?,
    };
    let result = serve_with_shutdown(listener, config, async {
        Err(DashboardError::Signal(std::io::Error::other("no signal")))
    })
    .await;
    assert!(matches!(result, Err(DashboardError::Signal(_))));
    Ok(())
}

#[test]
fn gateway_url_parsing() -> TestResult {
    let url: WsGatewayUrl = "wss://example.test/ws".parse()?;
    assert_eq!(url.to_string(), "wss://example.test/ws");
    assert!(matches!(
        WsGatewayUrl::parse("not a url"),
        Err(WsGatewayUrlError::Malformed(_))
    ));
    assert_eq!(
        WsGatewayUrl::parse("http://example.test/"),
        Err(WsGatewayUrlError::UnsupportedScheme("http".to_owned()))
    );
    assert!(matches!(
        WsGatewayUrl::parse("ws://"),
        Err(WsGatewayUrlError::Malformed(_))
    ));
    Ok(())
}

#[tokio::test]
async fn run_serves_until_shutdown() -> TestResult {
    let probe = TcpListener::bind("127.0.0.1:0").await?;
    let listen = probe.local_addr()?;
    drop(probe);
    let args = DashboardArgs::try_parse_from([
        "trogon-sandbox-dashboard",
        "--listen",
        &listen.to_string(),
        "--metrics-addr",
        "127.0.0.1:0",
    ])?;
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(run(args, async move {
        let _ = stopped.await;
        Ok(())
    }));
    let mut body = None;
    for _ in 0..50 {
        if let Ok(response) = reqwest::get(format!("http://{listen}/config.json")).await {
            body = Some(response.text().await?);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        body.ok_or("dashboard did not start")?
            .contains("ws://127.0.0.1:8443/v1/ws")
    );
    let _ = stop.send(());
    task.await??;
    Ok(())
}

#[tokio::test]
async fn run_reports_bind_failure() -> TestResult {
    let taken = TcpListener::bind("127.0.0.1:0").await?;
    let args = DashboardArgs::try_parse_from([
        "trogon-sandbox-dashboard",
        "--listen",
        &taken.local_addr()?.to_string(),
        "--metrics-addr",
        "127.0.0.1:0",
    ])?;
    let err = run(args, async { Ok(()) }).await.err().ok_or("expected bind failure")?;
    assert!(matches!(err, DashboardError::Bind(_)));
    assert!(!format!("{err}").is_empty());
    Ok(())
}

#[tokio::test]
async fn public_listener_serves_the_operational_paths() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let health = Health::new();
    let config = DashboardConfig {
        ws_gateway_url: WsGatewayUrl::parse("ws://127.0.0.1:8443/v1/ws")?,
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(serve_app_with_shutdown(
        listener,
        router(config).merge(health.router()),
        async move {
            let _ = stopped.await;
            Ok(())
        },
    ));

    let ready = format!("http://{addr}/-/readiness");
    assert_eq!(reqwest::get(&ready).await?.status(), 503);
    health.mark_started();
    assert_eq!(reqwest::get(&ready).await?.status(), 200);
    assert_eq!(reqwest::get(format!("http://{addr}/-/liveness")).await?.status(), 200);
    assert_eq!(reqwest::get(format!("http://{addr}/")).await?.status(), 200);

    let _ = stop.send(());
    task.await??;
    Ok(())
}
