#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use rmcp::ServiceExt;
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
async fn configured_binary_initializes_stdio_and_exits_when_client_disconnects() {
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
    let config = include_str!("../examples/config.toml").replace("http://127.0.0.1:50051", &endpoint);
    let path = std::env::temp_dir().join(format!("mcp-startup-{}.toml", std::process::id()));
    std::fs::write(&path, config).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcp-postgres"))
        .arg(&path)
        .env("MCP_POSTGRES_SPICEDB_TOKEN", "startup-test-token")
        .env("MCP_POSTGRES_ANALYTICS_DSN", &dsn)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = ().serve((child.stdout.take().unwrap(), child.stdin.take().unwrap())).await.unwrap();
    let tools = client.list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "query");
    client.cancel().await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).await.unwrap();
    assert!(!stderr.contains("startup-test-token"));
    assert!(!stderr.contains(&dsn));
    std::fs::remove_file(path).unwrap();
}
