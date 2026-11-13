#![cfg(unix)]

use std::{error::Error as StdError, io, process::Stdio};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use trogon_claude_agent_sdk::{
    Environment, InMemorySessionStore, SessionRuntime, SessionStoreOptions, StoreResume,
    adapters::{CurrentDirectory, SpawnProcess, SystemEnvironment, TokioProcessSpawner},
};

type TestResult = Result<(), Box<dyn StdError>>;

#[tokio::test]
async fn native_spawner_returns_the_spawned_child_and_its_pipes() -> TestResult {
    let mut command = Command::new("sh");
    command
        .args(["-c", "cat; exit 7"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = TokioProcessSpawner.spawn(&mut command)?;
    assert!(child.id().is_some());
    let mut stdin = child.stdin.take().ok_or("missing stdin")?;
    let mut stdout = child.stdout.take().ok_or("missing stdout")?;
    assert!(child.stderr.is_none());
    stdin.write_all(b"transport-message").await?;
    stdin.flush().await?;
    stdin.shutdown().await?;
    drop(stdin);
    let mut bytes = Vec::new();
    stdout.read_to_end(&mut bytes).await?;
    assert_eq!(bytes, b"transport-message");
    assert_eq!(child.wait().await?.code(), Some(7));
    assert_eq!(child.try_wait()?.ok_or("missing completed status")?.code(), Some(7));
    Ok(())
}

#[tokio::test]
async fn native_spawner_returns_the_spawn_error() -> TestResult {
    let mut command = Command::new("/nonexistent/claude-sdk-missing-executable");
    let error = TokioProcessSpawner
        .spawn(&mut command)
        .err()
        .ok_or("spawn unexpectedly succeeded")?;
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    Ok(())
}

#[tokio::test]
async fn native_environment_defaults_prepare_a_store_without_materializing_transcripts() -> TestResult {
    let directory = SystemEnvironment.current_dir()?;
    let store: InMemorySessionStore = InMemorySessionStore::default();
    let options = SessionStoreOptions::from(std::sync::Arc::new(store));
    let runtime = SessionRuntime::prepare(&options, StoreResume::None, &directory, &Environment::default()).await?;
    assert!(runtime.config_dir.is_none());
    assert!(runtime.projects_dir.is_absolute());
    assert!(runtime.materialized.is_none());
    assert!(runtime.resume_session_id.is_none());
    Ok(())
}
