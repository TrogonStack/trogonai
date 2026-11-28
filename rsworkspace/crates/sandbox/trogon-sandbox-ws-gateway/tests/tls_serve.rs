use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use rcgen::generate_simple_self_signed;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use trogon_sandbox_ws_gateway::{GatewayError, TlsFiles, serve_tls_with_shutdown};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Identity {
    _dir: tempfile::TempDir,
    files: TlsFiles,
    cert_pem: String,
}

fn identity() -> TestResult<Identity> {
    let generated = generate_simple_self_signed(vec!["localhost".to_string()])?;
    let dir = tempfile::tempdir()?;
    let cert_pem = generated.cert.pem();
    std::fs::write(dir.path().join("cert.pem"), &cert_pem)?;
    std::fs::write(dir.path().join("key.pem"), generated.signing_key.serialize_pem())?;
    let files = TlsFiles::new(dir.path().join("cert.pem"), dir.path().join("key.pem"));
    Ok(Identity {
        _dir: dir,
        files,
        cert_pem,
    })
}

fn app() -> Router {
    Router::new().route("/ping", get(|| async { "pong" }))
}

async fn get_over_tls(addr: SocketAddr, cert_pem: &str) -> TestResult<String> {
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(cert_pem.as_bytes())?)
        .resolve("localhost", addr)
        .build()?;
    let url = format!("https://localhost:{}/ping", addr.port());
    for _ in 0..100 {
        if let Ok(response) = client.get(&url).send().await {
            return Ok(response.text().await?);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("the TLS listener never answered".into())
}

#[tokio::test]
async fn serves_requests_over_tls_until_the_shutdown_resolves() -> TestResult {
    let identity = identity()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (stop, stopped) = oneshot::channel::<()>();
    let files = identity.files.clone();
    let server = tokio::spawn(async move {
        serve_tls_with_shutdown(listener, &files, app(), async move {
            let _ = stopped.await;
            Ok(())
        })
        .await
    });
    let body = get_over_tls(addr, &identity.cert_pem).await;
    let _ = stop.send(());
    server.await??;
    assert_eq!(body?, "pong");
    Ok(())
}

#[tokio::test]
async fn plain_http_clients_are_not_served_on_a_tls_listener() -> TestResult {
    let identity = identity()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (stop, stopped) = oneshot::channel::<()>();
    let files = identity.files.clone();
    let server = tokio::spawn(async move {
        serve_tls_with_shutdown(listener, &files, app(), async move {
            let _ = stopped.await;
            Ok(())
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let plain = reqwest::get(format!("http://{addr}/ping")).await;
    let _ = stop.send(());
    server.await??;
    assert!(plain.is_err());
    Ok(())
}

#[tokio::test]
async fn an_unreadable_certificate_is_a_tls_error() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let files = TlsFiles::new("/nonexistent/cert.pem", "/nonexistent/key.pem");
    let outcome = serve_tls_with_shutdown(listener, &files, app(), async { Ok(()) }).await;
    assert!(matches!(outcome, Err(GatewayError::Tls(_))));
    Ok(())
}
