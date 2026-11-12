#![cfg(all(windows, feature = "test-support"))]

use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::process::{Child, Command};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, CliPathError, Error,
    adapters::{NativeFileSystem, RuntimeAdapters, SpawnProcess, TokioProcessSpawner},
    test_support::FixedEnvironment,
};

/// Counts spawn calls and delegates them to the native spawner.
#[derive(Clone, Default)]
struct CountingSpawner(Arc<AtomicUsize>);

impl SpawnProcess for CountingSpawner {
    type Error = io::Error;

    fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        self.0.fetch_add(1, Ordering::SeqCst);
        TokioProcessSpawner.spawn(command)
    }
}

/// Refuse every Windows batch path before the process adapter can spawn anything.
#[tokio::test]
async fn windows_batch_cli_is_refused_before_spawn() -> Result<(), Box<dyn std::error::Error>> {
    for path in [
        "claude.bat",
        "CLAUDE.CMD",
        ".cmd",
        "C:claude.cmd",
        "claude:evil.cmd",
        "claude.cmd:stream",
        "claude.cmd. :stream",
        "claude.cmd   ...",
        "dir/claude.bat/../claude.exe",
        "claude.cmd\\...\\..",
        "dir//claude.cmd/./",
        "C:\\tools\\claude.BAT:stream.exe",
    ] {
        let process = CountingSpawner::default();
        let calls = process.0.clone();
        let result = ClaudeSdkClient::connect_with_adapters(
            ClaudeAgentOptions {
                cli_path: Some(path.into()),
                ..ClaudeAgentOptions::default()
            },
            RuntimeAdapters {
                environment: FixedEnvironment {
                    directory: PathBuf::from("unused-directory"),
                    variables: Default::default(),
                },
                process,
                filesystem: NativeFileSystem,
            },
        )
        .await;
        assert!(
            matches!(result, Err(Error::CliPath(CliPathError::WindowsBatchScript))),
            "{path}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}
