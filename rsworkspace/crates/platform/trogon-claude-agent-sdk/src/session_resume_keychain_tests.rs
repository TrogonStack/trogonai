#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{collections::BTreeMap, ffi::OsString, io, path::Path, process::Stdio};

use tokio::process::{Child, Command};

use std::error::Error as StdError;

type TestResult = Result<(), Box<dyn StdError>>;
use crate::{
    Environment, EnvironmentMode,
    adapters::{NativeFileSystem, SpawnProcess, TokioProcessSpawner},
    constants::KEYCHAIN_FALLBACK_ACCOUNT,
    session_error::SessionError,
    session_resume::{
        AuthSources, copy_auth_files, copy_auth_files_ts, keychain_service_ts, read_keychain_credentials,
    },
    test_support::FixedEnvironment,
};

/// Runs a real shell script in place of the `security` program, so the keychain path reads a real subprocess.
struct ShellProcess(&'static str);

impl SpawnProcess for ShellProcess {
    type Error = io::Error;

    fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let program = command.as_std().get_program().to_owned();
        let script = if program == "id" {
            "printf 'fixture-login\\n'"
        } else {
            assert_eq!(program, "security");
            self.0
        };
        let mut shell = Command::new("sh");
        shell
            .args(["-c", script])
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
            environment_mode: EnvironmentMode::default(),
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

fn fixed(variables: &[(&str, &str)]) -> FixedEnvironment {
    FixedEnvironment {
        directory: std::path::PathBuf::from("/"),
        variables: variables
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect(),
    }
}

/// Oracle: `p7("-credentials")` of the 0.3.323 bundle, run under node with HOME fixed to `/HOME`.
#[test]
fn typescript_keychain_service_names_match_the_bundle() {
    let cases: [(&[(&str, &str)], &str); 11] = [
        (&[("HOME", "/HOME")], "Claude Code-credentials"),
        (&[("CLAUDE_CONFIG_DIR", "/x/cfg")], "Claude Code-credentials-f0f6de85"),
        (
            &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/y/secure")],
            "Claude Code-credentials-828bce6f",
        ),
        (&[("CLAUDE_SECURESTORAGE_CONFIG_DIR", "")], "Claude Code-credentials"),
        (
            &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", ""), ("CLAUDE_CONFIG_DIR", "/x/cfg")],
            "Claude Code-credentials",
        ),
        (&[("CLAUDE_CONFIG_DIR", "")], "Claude Code-credentials"),
        (
            &[("CLAUDE_CONFIG_DIR", "/tmp/cafe\u{301}")],
            "Claude Code-credentials-0873cca0",
        ),
        (
            &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/tmp/cafe\u{301}")],
            "Claude Code-credentials-0873cca0",
        ),
        (
            &[("CLAUDE_CODE_CUSTOM_OAUTH_URL", "https://claude.fedstart.com/")],
            "Claude Code-custom-oauth-credentials",
        ),
        (
            &[
                ("CLAUDE_CODE_CUSTOM_OAUTH_URL", "https://claude.fedstart.com"),
                ("CLAUDE_CONFIG_DIR", "/x/cfg"),
            ],
            "Claude Code-custom-oauth-credentials-f0f6de85",
        ),
        (
            &[
                ("CLAUDE_CODE_CUSTOM_OAUTH_URL", "https://claude.fedstart.com"),
                ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/y/secure"),
            ],
            "Claude Code-custom-oauth-credentials-828bce6f",
        ),
    ];
    for (variables, expected) in cases {
        assert_eq!(
            keychain_service_ts(&fixed(variables)).as_deref().ok(),
            Some(expected),
            "{variables:?}"
        );
    }
}

#[test]
fn typescript_keychain_service_rejects_an_unapproved_oauth_endpoint() {
    let environment = fixed(&[("CLAUDE_CODE_CUSTOM_OAUTH_URL", "https://example.invalid")]);
    assert!(matches!(
        keychain_service_ts(&environment),
        Err(SessionError::UnapprovedOauthEndpoint)
    ));
    assert_eq!(
        SessionError::UnapprovedOauthEndpoint.to_string(),
        "CLAUDE_CODE_CUSTOM_OAUTH_URL is not an approved endpoint."
    );
}

/// Records the arguments the lookup passes and answers with credentials.
struct RecordingProcess(std::path::PathBuf);

impl SpawnProcess for RecordingProcess {
    type Error = io::Error;

    fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let mut shell = Command::new("sh");
        shell
            .arg("-c")
            .arg(r#"printf '%s\n' "$@" > "$0"; printf '{"claudeAiOauth":{"accessToken":"a"}}'"#)
            .arg(&self.0)
            .args(command.as_std().get_args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        TokioProcessSpawner.spawn(&mut shell)
    }
}

/// Fails the login name lookup (`id -un`) and records the arguments the keychain lookup passes.
struct NoLoginName(std::path::PathBuf);

impl SpawnProcess for NoLoginName {
    type Error = io::Error;

    fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        if command.as_std().get_program() != "id" {
            return RecordingProcess(self.0.clone()).spawn(command);
        }
        let mut shell = Command::new("sh");
        shell
            .args(["-c", "exit 1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        TokioProcessSpawner.spawn(&mut shell)
    }
}

/// Without `USER` and with no passwd name to read, both SDKs ask the keychain for the fallback account.
#[tokio::test]
async fn a_failed_login_name_lookup_uses_the_fallback_account() -> TestResult {
    for typescript in [false, true] {
        let directory = tempfile::tempdir()?;
        let log = directory.path().join("arguments");
        let environment = fixed(&[("HOME", "/HOME")]);
        let credentials =
            read_keychain_credentials(&environment, &NoLoginName(log.clone()), "service", typescript).await;
        assert!(credentials.is_some(), "typescript {typescript}");
        let arguments = std::fs::read_to_string(&log)?;
        assert_eq!(
            arguments,
            format!("find-generic-password\n-a\n{KEYCHAIN_FALLBACK_ACCOUNT}\n-w\n-s\nservice\n"),
            "typescript {typescript}"
        );
    }
    Ok(())
}

struct Run {
    credentials: Option<Vec<u8>>,
    arguments: Option<String>,
}

async fn run_typescript(
    environment: &FixedEnvironment,
    subprocess: &Environment,
    mode: EnvironmentMode,
) -> Result<Run, Box<dyn StdError>> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    std::fs::create_dir_all(&source)?;
    std::fs::create_dir_all(&destination)?;
    let log = directory.path().join("arguments");
    copy_auth_files_ts(
        &NativeFileSystem,
        &destination,
        AuthSources {
            caller_config_dir: None,
            source_config_dir: &source,
            subprocess_environment: subprocess,
            environment_mode: mode,
            environment,
            process: &RecordingProcess(log.clone()),
            keychain: true,
        },
    )
    .await?;
    Ok(Run {
        credentials: read_optional(&destination.join(".credentials.json"))?,
        arguments: std::fs::read_to_string(&log).ok(),
    })
}

#[tokio::test]
async fn typescript_lookup_passes_the_account_and_the_hashed_service() -> TestResult {
    let environment = fixed(&[
        ("HOME", "/HOME"),
        ("USER", "fixture-user"),
        ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/y/secure"),
    ]);
    let run = run_typescript(&environment, &Environment::default(), EnvironmentMode::Replace).await?;
    assert_eq!(
        run.arguments.as_deref(),
        Some("find-generic-password\n-a\nfixture-user\n-w\n-s\nClaude Code-credentials-828bce6f\n")
    );
    assert!(run.credentials.is_some());
    Ok(())
}

#[tokio::test]
async fn typescript_lookup_falls_back_for_an_account_the_cli_would_reject() -> TestResult {
    let environment = fixed(&[("HOME", "/HOME"), ("USER", "bad user")]);
    let run = run_typescript(&environment, &Environment::default(), EnvironmentMode::Replace).await?;
    let arguments = run.arguments.unwrap_or_default();
    assert!(arguments.contains("\nclaude-code-user\n"), "{arguments}");
    Ok(())
}

/// TypeScript `options.env` replaces `process.env`: auth variables only count where the CLI will see them.
#[tokio::test]
async fn replace_mode_ignores_auth_variables_of_this_process() -> TestResult {
    let environment = fixed(&[("HOME", "/HOME"), ("ANTHROPIC_API_KEY", "process-key")]);
    let replaced = run_typescript(&environment, &Environment::default(), EnvironmentMode::Replace).await?;
    assert!(
        replaced.arguments.is_some(),
        "keychain consulted: the key is not in options.env"
    );
    for mode in [EnvironmentMode::Overlay, EnvironmentMode::Inherit] {
        let merged = run_typescript(&environment, &Environment::default(), mode).await?;
        assert!(
            merged.arguments.is_none(),
            "{mode:?}: the subprocess still inherits the key"
        );
    }
    let given = Environment::from([("ANTHROPIC_API_KEY".into(), "option-key".into())]);
    let bare = fixed(&[("HOME", "/HOME")]);
    for mode in [
        EnvironmentMode::Replace,
        EnvironmentMode::Overlay,
        EnvironmentMode::Inherit,
    ] {
        assert!(
            run_typescript(&bare, &given, mode).await?.arguments.is_none(),
            "{mode:?}"
        );
    }
    Ok(())
}

#[cfg(unix)]
async fn copied_modes(typescript: bool, mode: u32) -> Result<u32, Box<dyn StdError>> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    std::fs::create_dir_all(&source)?;
    std::fs::create_dir_all(&destination)?;
    std::fs::write(directory.path().join(".claude.json"), "{}")?;
    std::fs::set_permissions(
        directory.path().join(".claude.json"),
        std::fs::Permissions::from_mode(mode),
    )?;
    let environment = fixed(&[("HOME", directory.path().to_str().unwrap_or("/"))]);
    let sources = AuthSources {
        caller_config_dir: None,
        source_config_dir: &source,
        subprocess_environment: &Environment::default(),
        environment_mode: EnvironmentMode::default(),
        environment: &environment,
        process: &MissingProcess,
        keychain: false,
    };
    if typescript {
        copy_auth_files_ts(&NativeFileSystem, &destination, sources).await?;
    } else {
        copy_auth_files(&NativeFileSystem, &destination, sources).await?;
    }
    Ok(std::fs::metadata(destination.join(".claude.json"))?
        .permissions()
        .mode()
        & 0o777)
}

/// Oracle: node's `fs.copyFile` under umask 077 gives 640 -> 640, 755 -> 755, 444 -> 444, 600 -> 600 and
/// 666 -> 666, while Python's `write_bytes` plus `chmod(0o600)` gives 600 for every source.
#[cfg(unix)]
#[tokio::test]
async fn typescript_keeps_the_source_mode_and_python_forces_0600() -> TestResult {
    for mode in [0o640, 0o755, 0o444, 0o600, 0o666] {
        assert_eq!(copied_modes(true, mode).await?, mode, "typescript {mode:o}");
        assert_eq!(copied_modes(false, mode).await?, 0o600, "python {mode:o}");
    }
    Ok(())
}
