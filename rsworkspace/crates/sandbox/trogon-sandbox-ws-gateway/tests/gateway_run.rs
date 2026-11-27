mod support;

use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{ApiKeyBucket, NatsKvApiKeyStore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};
use trogon_sandbox_ws_gateway::{API_KEY_HEADER, GatewayArgs, GatewayError, run};

use support::TestResult;

async fn free_addr() -> TestResult<SocketAddr> {
    let probe = TcpListener::bind("127.0.0.1:0").await?;
    let addr = probe.local_addr()?;
    drop(probe);
    Ok(addr)
}

fn args(nats_url: &str, listen: SocketAddr, extra: &[&str]) -> TestResult<GatewayArgs> {
    let mut flags = vec![
        "trogon-sandbox-ws-gateway".to_string(),
        "--nats-url".to_string(),
        nats_url.to_string(),
        "--listen".to_string(),
        listen.to_string(),
        "--metrics-addr".to_string(),
        "127.0.0.1:0".to_string(),
    ];
    flags.extend(extra.iter().map(|flag| flag.to_string()));
    Ok(GatewayArgs::try_parse_from(flags)?)
}

async fn session_status(listen: SocketAddr, key: &str) -> TestResult<u16> {
    let client = reqwest::Client::new();
    for _ in 0..100 {
        let attempt = client
            .post(format!("http://{listen}/v1/sessions"))
            .header(API_KEY_HEADER, key)
            .send()
            .await;
        if let Ok(response) = attempt {
            return Ok(response.status().as_u16());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("the gateway did not listen".into())
}

async fn mint(server: &JetStreamTestServer, bucket: &ApiKeyBucket) -> TestResult<String> {
    let store = NatsKvApiKeyStore::new_with_bucket(server.jetstream().await, bucket).await?;
    let key = store
        .mint(TenantId::new("tenant-a")?, ServiceAccountId::new("sa-1"), "run")
        .await?;
    Ok(key.expose_secret().to_string())
}

async fn serve_and_stop(server: &JetStreamTestServer, extra: Vec<String>) -> TestResult {
    let key = mint(server, &ApiKeyBucket::default()).await?;
    let listen = free_addr().await?;
    let url = format!("nats://{}", server.address());
    let refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    let (stop, stopped) = oneshot::channel::<()>();
    let gateway = tokio::spawn(run(args(&url, listen, &refs)?, async move {
        let _ = stopped.await;
        Ok(())
    }));
    let status = session_status(listen, &key).await;
    let _ = stop.send(());
    gateway.await??;
    assert_eq!(status?, 201);
    Ok(())
}

#[tokio::test]
async fn run_serves_sessions_until_the_shutdown_resolves() -> TestResult {
    serve_and_stop(&JetStreamTestServer::start().await, vec![]).await
}

#[tokio::test]
async fn run_reads_keys_from_a_separate_store_and_a_named_bucket() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let bucket = ApiKeyBucket::new("gateway-api-keys")?;
    let key = mint(&server, &bucket).await?;
    let other = JetStreamTestServer::start().await;
    let listen = free_addr().await?;
    let service_url = format!("nats://{}", other.address());
    let store_url = format!("nats://{}", server.address());
    let (stop, stopped) = oneshot::channel::<()>();
    let gateway = tokio::spawn(run(
        args(
            &service_url,
            listen,
            &[
                "--api-key-store-url",
                &store_url,
                "--api-key-bucket",
                "gateway-api-keys",
            ],
        )?,
        async move {
            let _ = stopped.await;
            Ok(())
        },
    ));
    let status = session_status(listen, &key).await;
    let _ = stop.send(());
    gateway.await??;
    assert_eq!(status?, 201);
    Ok(())
}

#[tokio::test]
async fn run_accepts_a_store_url_equal_to_the_service_url() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    serve_and_stop(&server, vec!["--api-key-store-url".to_string(), url]).await
}

#[tokio::test]
async fn a_shutdown_failure_is_the_result_of_run() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let listen = free_addr().await?;
    let url = format!("nats://{}", server.address());
    let outcome = run(args(&url, listen, &[])?, async {
        Err(GatewayError::Signal(std::io::Error::other("no signal handler")))
    })
    .await;
    assert!(matches!(outcome, Err(GatewayError::Signal(_))));
    Ok(())
}

#[tokio::test]
async fn an_occupied_listen_address_is_a_bind_error() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let occupied = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("nats://{}", server.address());
    let outcome = run(args(&url, occupied.local_addr()?, &[])?, async { Ok(()) }).await;
    assert!(matches!(outcome, Err(GatewayError::Bind(_))));
    Ok(())
}

#[tokio::test]
async fn an_unparsable_nats_url_is_a_connect_error() -> TestResult {
    let outcome = run(args("not a url", free_addr().await?, &[])?, async { Ok(()) }).await;
    assert!(matches!(outcome, Err(GatewayError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn an_unusable_separate_store_url_is_a_connect_error() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let outcome = run(
        args(&url, free_addr().await?, &["--api-key-store-url", "not a url"])?,
        async { Ok(()) },
    )
    .await;
    assert!(matches!(outcome, Err(GatewayError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn an_invalid_bucket_name_is_refused_when_parsing_the_flags() -> TestResult {
    let parsed = args(
        "nats://127.0.0.1:1",
        free_addr().await?,
        &["--api-key-bucket", "invalid.bucket"],
    );
    assert!(parsed.is_err());
    Ok(())
}
