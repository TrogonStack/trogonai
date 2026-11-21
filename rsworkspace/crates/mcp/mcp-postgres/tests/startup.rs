#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use std::{
    process::{Command, Stdio},
    time::Duration,
};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    },
};
use tokio::io::AsyncReadExt;

#[test]
fn invalid_startup_reports_only_safe_failure() {
    for arguments in [
        vec![],
        vec!["missing-private-config", "extra"],
        vec!["missing-private-config"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mcp-postgres"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("startup or transport failed"));
        assert!(!stderr.contains("missing-private-config"));
    }
}

#[tokio::test]
async fn configured_binary_authenticates_http_and_stops_on_termination() {
    let postgres = Postgres::default().with_tag("18.3-alpine").start().await.unwrap();
    let spicedb = GenericImage::new("authzed/spicedb", "v1.53.0")
        .with_exposed_port(50051.tcp())
        .with_wait_for(WaitFor::message_on_either_std("grpc server started serving"))
        .with_cmd([
            "serve",
            "--datastore-engine",
            "memory",
            "--grpc-preshared-key",
            "startup-test-token",
        ])
        .start()
        .await
        .unwrap();
    let endpoint = format!(
        "http://{}:{}",
        spicedb.get_host().await.unwrap(),
        spicedb.get_host_port_ipv4(50051.tcp()).await.unwrap()
    );
    let dsn = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        postgres.get_host().await.unwrap(),
        postgres.get_host_port_ipv4(5432).await.unwrap()
    );
    let issuer = common::TestIssuer::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bind = listener.local_addr().unwrap();
    drop(listener);
    let resource = format!("http://{bind}/mcp");
    let config = include_str!("../examples/config.toml")
        .replace("http://127.0.0.1:50051", &endpoint)
        .replace("https://identity.example.com/.well-known/jwks.json", &issuer.jwks_uri)
        .replace("https://identity.example.com", &issuer.issuer)
        .replace("https://sql.example.com/mcp", &resource)
        .replace("0.0.0.0:8080", &bind.to_string())
        .replace("external-user-id", "test-user")
        .replace("external-client-id", "test-client");
    let path = std::env::temp_dir().join(format!("mcp-startup-{}.toml", std::process::id()));
    std::fs::write(&path, config).unwrap();
    for signal in ["-TERM", "-INT"] {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcp-postgres"))
            .arg(&path)
            .env("MCP_POSTGRES_SPICEDB_TOKEN", "startup-test-token")
            .env("MCP_POSTGRES_ANALYTICS_DSN", &dsn)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let http = reqwest::Client::new();
        let metadata = format!("http://{bind}/.well-known/oauth-protected-resource/mcp");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if http
                    .get(&metadata)
                    .send()
                    .await
                    .is_ok_and(|response| response.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            http.post(&resource).send().await.unwrap().status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        let mut claims = issuer.claims();
        claims["aud"] = serde_json::json!(resource);
        let token = issuer.token_with_claims(claims);
        let response = http
            .post(&resource)
            .bearer_auth(&token)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let tools: serde_json::Value = response.json().await.unwrap();
        assert_eq!(tools["result"]["tools"][0]["name"], "query");
        assert!(
            Command::new("kill")
                .args([signal, &child.id().unwrap().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
        let mut stderr = String::new();
        child.stderr.take().unwrap().read_to_string(&mut stderr).await.unwrap();
        assert!(!stderr.contains("startup-test-token"));
        assert!(!stderr.contains(&dsn));
        assert!(!stderr.contains(&token));
        let mut stdout = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut stdout).await.unwrap();
        assert!(stdout.is_empty());
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn startup_requires_http_and_authentication_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    for missing in ["http", "authentication"] {
        let mut config: toml::Value = toml::from_str(include_str!("../examples/config.toml")).unwrap();
        config.as_table_mut().unwrap().remove(missing);
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_mcp-postgres"))
            .arg(&path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("startup or transport failed"));
        assert!(!stderr.contains("identity.example.com"));
    }
}
