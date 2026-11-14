#![cfg(feature = "test-support")]

#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};
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
    ClaudeAgentOptions, ClaudeSdkClient, Error, WindowsBatchCli,
    adapters::{NativeFileSystem, RuntimeAdapters, SpawnProcess},
    test_support::FixedEnvironment,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// What Python's `_reject_windows_batch_cli` did with each path, captured from the real package.
const GOLDENS: &str = include_str!("fixtures/python_text/goldens.json");

/// Python's `platform.system()` on this host.
#[cfg(windows)]
const SYSTEM: &str = "Windows";
#[cfg(target_os = "macos")]
const SYSTEM: &str = "Darwin";
#[cfg(not(any(windows, target_os = "macos")))]
const SYSTEM: &str = "Linux";

/// Counts spawn calls and answers each one as a missing executable, so nothing is ever started.
#[derive(Clone, Default)]
struct CountingSpawner(Arc<AtomicUsize>);

impl SpawnProcess for CountingSpawner {
    type Error = io::Error;

    fn spawn(&self, _: &mut Command) -> io::Result<Child> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(io::ErrorKind::NotFound.into())
    }
}

/// Connects with `cli_path` and returns the result together with how many spawns reached the process adapter.
async fn connect(cli_path: PathBuf, windows_batch_cli: WindowsBatchCli) -> (Result<ClaudeSdkClient, Error>, usize) {
    let process = CountingSpawner::default();
    let calls = process.0.clone();
    let result = ClaudeSdkClient::connect_with_adapters(
        ClaudeAgentOptions {
            cli_path: Some(cli_path),
            windows_batch_cli,
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
    (result, calls.load(Ordering::SeqCst))
}

/// The golden paths that are valid on this host, each with Python's refusal text for this host's `platform.system()`.
fn golden_paths() -> Result<Vec<(PathBuf, Option<String>)>, Box<dyn std::error::Error>> {
    let goldens: serde_json::Value = serde_json::from_str(GOLDENS)?;
    let mut paths = Vec::new();
    for case in goldens["windows_batch_cli"]
        .as_array()
        .ok_or("no windows_batch_cli goldens")?
    {
        let hex = case["path"].as_str().ok_or("no path")?;
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(hex.get(index..index + 2).unwrap_or_default(), 16))
            .collect::<Result<Vec<u8>, _>>()?;
        let refusal = case[SYSTEM].as_str().map(str::to_owned);
        paths.extend(host_path(bytes).map(|path| (path, refusal)));
    }
    Ok(paths)
}

#[cfg(unix)]
fn host_path(bytes: Vec<u8>) -> Option<PathBuf> {
    Some(OsString::from_vec(bytes).into())
}

/// Windows paths are UTF-16, so the one golden path that is not UTF-8 has no Windows twin.
#[cfg(not(unix))]
fn host_path(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Python checks `platform.system()` first, so off Windows the refusal never fires and every path is spawned.
#[cfg(not(windows))]
#[tokio::test]
async fn the_python_refusal_spawns_off_windows_as_python_does() -> TestResult {
    let paths = golden_paths()?;
    assert_eq!(paths.len(), 28);
    for (path, refusal) in paths {
        assert_eq!(refusal, None, "{}", path.display());
        let (result, calls) = connect(path.clone(), WindowsBatchCli::Refuse).await;
        assert!(matches!(result, Err(Error::CliNotFoundAt(_))), "{}", path.display());
        assert_eq!(calls, 1, "{}", path.display());
    }
    Ok(())
}

/// With the Python refusal selected, refuse every Windows batch path with Python's text before the process adapter
/// can spawn anything.
#[cfg(windows)]
#[tokio::test]
async fn windows_batch_cli_is_refused_before_spawn() -> TestResult {
    let paths = golden_paths()?;
    assert_eq!(paths.len(), 27);
    for (path, refusal) in paths {
        let (result, calls) = connect(path.clone(), WindowsBatchCli::Refuse).await;
        assert_eq!(
            result.err().and_then(|error| match error {
                Error::CliPath(error) => Some(error.to_string()),
                _ => None,
            }),
            refusal,
            "{}",
            path.display()
        );
        assert_eq!(calls, usize::from(refusal.is_none()), "{}", path.display());
    }
    Ok(())
}

/// The TypeScript SDK spawns batch scripts, so the default hands them to the process adapter.
#[tokio::test]
async fn windows_batch_cli_is_spawned_by_default() -> TestResult {
    let (result, calls) = connect("missing-fixture-claude.cmd".into(), WindowsBatchCli::default()).await;
    assert!(matches!(result, Err(Error::CliNotFoundAt(_))));
    assert_eq!(calls, 1);
    Ok(())
}
