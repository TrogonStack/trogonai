use std::{collections::BTreeMap, ffi::OsString, io, path::Path, process::Stdio};

use tokio::process::{Child, Command};

use std::error::Error as StdError;

type TestResult = Result<(), Box<dyn StdError>>;
use crate::{
    Environment,
    adapters::{NativeFileSystem, SpawnProcess, TokioProcessSpawner},
    session_resume::{AuthSources, copy_auth_files},
    test_support::FixedEnvironment,
};

/// Runs a real shell script in place of the `security` program, so the keychain path reads a real subprocess.
struct ShellProcess(&'static str);

impl SpawnProcess for ShellProcess {
    type Error = io::Error;

    fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        assert_eq!(command.as_std().get_program(), "security");
        let mut shell = Command::new("sh");
        shell
            .args(["-c", self.0])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        TokioProcessSpawner.spawn(&mut shell)
    }
}

/// Spawns a program that does not exist.
struct MissingProcess;

impl SpawnProcess for MissingProcess {
    type Error = io::Error;

    fn spawn(&self, _: &mut Command) -> io::Result<Child> {
        TokioProcessSpawner.spawn(&mut Command::new("trogon-claude-agent-sdk-missing-program"))
    }
}

async fn credentials_from<P>(
    process: &P,
    user: Option<&str>,
    keychain: bool,
) -> Result<Option<Vec<u8>>, Box<dyn StdError>>
where
    P: SpawnProcess<Error = io::Error>,
{
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    std::fs::create_dir_all(&source)?;
    std::fs::create_dir_all(&destination)?;
    let mut variables = BTreeMap::new();
    variables.insert(OsString::from("HOME"), directory.path().as_os_str().to_owned());
    if let Some(user) = user {
        variables.insert(OsString::from("USER"), OsString::from(user));
    }
    let environment = FixedEnvironment {
        directory: directory.path().to_path_buf(),
        variables,
    };
    let subprocess_environment = Environment::default();
    copy_auth_files(
        &NativeFileSystem,
        &destination,
        AuthSources {
            caller_config_dir: None,
            source_config_dir: &source,
            subprocess_environment: &subprocess_environment,
            environment: &environment,
            process,
            keychain,
        },
    )
    .await?;
    let written = destination.join(".credentials.json");
    Ok(read_optional(&written)?)
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

const OAUTH: &str = r#"printf ' {"claudeAiOauth":{"accessToken":"a","refreshToken":"r"}}\n'"#;

#[tokio::test]
async fn keychain_credentials_are_trimmed_redacted_and_written() -> TestResult {
    let written = credentials_from(&ShellProcess(OAUTH), Some("fixture-user"), true).await?;
    assert_eq!(written, Some(br#"{"claudeAiOauth":{"accessToken":"a"}}"#.to_vec()));
    Ok(())
}

#[tokio::test]
async fn keychain_lookup_without_a_user_uses_the_fallback_account() -> TestResult {
    let written = credentials_from(&ShellProcess(OAUTH), None, true).await?;
    assert!(written.is_some());
    Ok(())
}

#[tokio::test]
async fn keychain_is_not_consulted_when_the_platform_has_none() -> TestResult {
    assert_eq!(credentials_from(&ShellProcess(OAUTH), None, false).await?, None);
    Ok(())
}

#[tokio::test]
async fn keychain_failures_leave_credentials_unwritten() -> TestResult {
    assert_eq!(credentials_from(&MissingProcess, None, true).await?, None);
    for script in ["exit 1", "printf '\\377'", "printf '  \\n'"] {
        assert_eq!(credentials_from(&ShellProcess(script), None, true).await?, None);
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_hung_keychain_lookup_times_out() -> TestResult {
    assert_eq!(credentials_from(&ShellProcess("sleep 3600"), None, true).await?, None);
    Ok(())
}
