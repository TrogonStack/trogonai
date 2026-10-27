#![cfg(unix)]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const DEADLINE: Duration = Duration::from_secs(15);

fn command(log_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-provider-gateway"));
    command
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:9")
        .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://127.0.0.1:9/v1/traces")
        .env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "http://127.0.0.1:9/v1/metrics")
        .env("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "http://127.0.0.1:9/v1/logs")
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json")
        .env("TROGON_LOG_DIR", log_dir)
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn startup(child: &mut Child) -> (SocketAddr, JoinHandle<String>) {
    let stderr = child.stderr.take().unwrap();
    let (address_tx, address_rx) = oneshot::channel();
    let output = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut address_tx = Some(address_tx);
        let mut output = String::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && value["fields"]["message"] == "provider gateway listening"
                && let Some(address) = value["fields"]["address"].as_str()
                && let Ok(address) = address.parse::<SocketAddr>()
                && let Some(sender) = address_tx.take()
            {
                sender.send(address).unwrap();
            }
            output.push_str(&line);
            output.push('\n');
        }
        output
    });
    match address_rx.await {
        Ok(address) => (address, output),
        Err(error) => panic!(
            "gateway exited before announcing its listener: {error}; {}",
            output.await.unwrap()
        ),
    }
}

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
async fn binary_serves_probes_and_exits_cleanly_on_sigterm() {
    timeout(DEADLINE, async {
        let logs = tempfile::tempdir().unwrap();
        let mut child = command(logs.path())
            .args(["--host", "127.0.0.1", "--port", "0"])
            .spawn()
            .unwrap();
        let (address, output) = startup(&mut child).await;
        assert_ne!(address.port(), 0);
        for path in ["/-/liveness", "/-/readiness"] {
            let response = request(address, path).await;
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
            assert_eq!(response.split_once("\r\n\r\n").unwrap().1, "");
        }
        assert!(
            Command::new("/bin/kill")
                .args(["-TERM", &child.id().unwrap().to_string()])
                .status()
                .await
                .unwrap()
                .success()
        );
        let status = child.wait().await.unwrap();
        let output = output.await.unwrap();
        assert!(status.success(), "{status}: {output}");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn binary_reports_an_occupied_listener() {
    timeout(DEADLINE, async {
        let logs = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let output = command(logs.path())
            .args(["--host", "127.0.0.1", "--port", &address.port().to_string()])
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(&address.to_string()), "{stderr}");
        assert!(stderr.contains("AddrInUse"), "{stderr}");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn binary_rejects_invalid_configuration_before_serving() {
    timeout(DEADLINE, async {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("invalid.toml");
        std::fs::write(&config, "host = [").unwrap();
        let output = command(directory.path())
            .arg("--config")
            .arg(&config)
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("ParseFile"), "{stderr}");
        assert!(stderr.contains("invalid.toml"), "{stderr}");
        assert!(!stderr.contains("provider gateway listening"), "{stderr}");
    })
    .await
    .unwrap();
}
