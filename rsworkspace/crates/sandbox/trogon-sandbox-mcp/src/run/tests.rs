use clap::Parser;
use rmcp::ServiceExt;
use trogon_std::env::InMemoryEnv;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{build_policy, connect, run};
use crate::config::Config;
use crate::run_error::RunError;
use crate::testing::TestResult;

const UNREACHABLE: &str = "nats://127.0.0.1:1";

fn config(extra: &[&str]) -> Result<Config, clap::Error> {
    let mut args = vec!["trogon-sandbox-mcp", "--tenant", "acme", "--nats-url", UNREACHABLE];
    args.extend_from_slice(extra);
    Config::try_parse_from(args)
}

#[test]
fn the_policy_follows_the_flags() -> TestResult {
    let policy = build_policy(&config(&[
        "--deny-tool",
        "sandbox_x",
        "--deny-command",
        "rm",
        "--root",
        "/work",
    ])?)?;
    assert!(!policy.tools.is_empty());
    assert!(!policy.commands.is_empty());
    assert!(!policy.roots.is_empty());
    assert!(build_policy(&config(&[])?)?.tools.is_empty());
    Ok(())
}

#[test]
fn a_bad_command_pattern_is_refused() -> TestResult {
    let result = build_policy(&config(&["--deny-command", "("])?);
    assert!(matches!(result, Err(RunError::Commands(_))));
    Ok(())
}

#[test]
fn a_relative_root_is_refused() -> TestResult {
    let result = build_policy(&config(&["--root", "work"])?);
    assert!(matches!(result, Err(RunError::Roots(_))));
    Ok(())
}

#[tokio::test]
async fn an_unreachable_broker_leaves_no_client() {
    assert!(connect(UNREACHABLE, Some("key")).await.is_none());
    assert!(connect(UNREACHABLE, None).await.is_none());
}

#[tokio::test]
async fn the_server_runs_until_the_client_leaves() -> TestResult {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let config = config(&["--api-key", "k", "--allow-tool", "nothing_*"])?;
    let env = InMemoryEnv::new();
    let serving = run(config, &env, server_io);
    let using = async {
        let client = ().serve(client_io).await?;
        let tools = client.list_all_tools().await?;
        client.cancel().await?;
        Ok::<_, Box<dyn std::error::Error>>(tools)
    };
    let (served, tools) = tokio::join!(serving, using);
    served?;
    assert!(tools?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_bad_policy_stops_the_server_before_it_serves() -> TestResult {
    let (server_io, _client_io) = tokio::io::duplex(1024);
    let env = InMemoryEnv::new();
    let result = run(config(&["--root", "work"])?, &env, server_io).await;
    assert!(matches!(result, Err(RunError::Roots(_))));
    Ok(())
}

#[tokio::test]
async fn an_insecure_key_file_stops_the_server_before_it_serves() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir()?;
    let dir = home.path().join(".config").join("trogon-sandbox-mcp");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("api-key");
    std::fs::write(&file, "secret")?;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644))?;
    let env = InMemoryEnv::new();
    env.set("HOME", home.path().to_string_lossy().to_string());
    let (server_io, _client_io) = tokio::io::duplex(1024);
    let result = run(config(&[])?, &env, server_io).await;
    assert!(matches!(result, Err(RunError::ApiKey(_))));
    Ok(())
}

/// A minimal NATS server: it greets, answers the client's ping, and then keeps the connection open.
async fn fake_nats() -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("nats://{}", listener.local_addr()?);
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let info = "INFO {\"server_id\":\"fake\",\"version\":\"2.10.0\",\"proto\":1,\"max_payload\":1048576,\"headers\":true}\r\n";
        let _ = socket.write_all(info.as_bytes()).await;
        let mut seen = Vec::new();
        let mut chunk = [0u8; 1024];
        while let Ok(read) = socket.read(&mut chunk).await {
            if read == 0 {
                break;
            }
            seen.extend_from_slice(&chunk[..read]);
            if seen.ends_with(b"PING\r\n") {
                let _ = socket.write_all(b"PONG\r\n").await;
            }
        }
    });
    Ok(url)
}

fn quiet_logging() -> tracing::subscriber::DefaultGuard {
    tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(std::io::sink).finish())
}

#[tokio::test]
async fn a_reachable_broker_gives_a_client() -> TestResult {
    let _logging = quiet_logging();
    let url = fake_nats().await?;
    assert!(connect(&url, Some("key")).await.is_some());
    let url = fake_nats().await?;
    assert!(connect(&url, None).await.is_some());
    Ok(())
}

#[tokio::test]
async fn an_active_policy_is_logged_when_the_server_starts() -> TestResult {
    let _logging = quiet_logging();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let config = config(&["--api-key", "k", "--deny-tool", "sandbox_x"])?;
    let env = InMemoryEnv::new();
    let serving = run(config, &env, server_io);
    let using = async {
        let client = ().serve(client_io).await?;
        client.cancel().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (served, used) = tokio::join!(serving, using);
    served?;
    used
}
