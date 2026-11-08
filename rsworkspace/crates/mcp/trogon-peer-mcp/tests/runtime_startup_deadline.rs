#![cfg(unix)]

use std::io::Write as _;
use std::time::Duration;

use trogon_peer_mcp::{RuntimeConfig, RuntimeError, run_stdio};
use trogon_std::env::InMemoryEnv;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn startup_budget_includes_a_delayed_configuration_source() -> TestResult {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("delayed-source");
    let created = std::process::Command::new("mkfifo").arg("-m").arg("600").arg(&source).status()?;
    assert!(created.success(), "could not create bounded delayed source");
    let mut writer = std::fs::OpenOptions::new().read(true).write(true).open(&source)?;
    let env = InMemoryEnv::new();
    for (key, value) in [
        ("PEER_TENANT_ID", "tenant"),
        ("PEER_ACCOUNT_MODE", "shared_fixture"),
        ("PEER_CALLER_ID", "alice"),
        ("PEER_CONNECTION_ID", "one"),
        ("PEER_INSTALLATION_ID", "install"),
        ("PEER_COLLABORATION_REF", "shared"),
        ("PEER_DISPLAY_NAME", "alice"),
        ("PEER_AVAILABILITY", "ready"),
    ] {
        env.set(key, value);
    }
    env.set("NATS_CREDS", format!("file://{}", source.display()));
    let config = RuntimeConfig::from_env(&env)?;
    let release_source = async move {
        tokio::time::sleep(Duration::from_millis(2250)).await;
        writer.write_all(b"synthetic invalid fixture input")?;
        drop(writer);
        Ok::<(), std::io::Error>(())
    };
    let (outcome, released) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(run_stdio(config, std::future::pending()), release_source)
    }).await?;
    released?;
    assert!(matches!(outcome, Err(RuntimeError::StartupDeadline)),
        "configuration loading escaped the two-second startup budget: {outcome:?}");
    Ok(())
}
