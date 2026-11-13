#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Adapter tests exercise exact protocol frames through a scripted child process."
    )
)]

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, InMemorySessionStore, McpRemoteServerOptions, McpServerName, McpServerOptions,
    McpServers, PermissionMode, Prompt, SdkMcpServers, SessionEntry, SessionError, SessionId, SessionKey,
    SessionProjectKey, SessionStore, SessionStoreOptions, SettingSource,
    adapters::{
        CurrentDirectory, EnvironmentVariable, NativeFileSystem, RuntimeAdapters, TemporaryDirectory, WallClock,
    },
    test_support::FixedEnvironment,
};

#[path = "support/recording_filesystem.rs"]
mod recording_filesystem;
#[path = "support/scripted_process.rs"]
mod scripted_process;

use recording_filesystem::RecordingFileSystem;
use scripted_process::{
    CapturedProcessCommand, MissingPipe, ScriptedProcessEvent, ScriptedProcessHarness, ScriptedProcessSpawner,
};

struct FrozenClock;

impl WallClock for FrozenClock {
    type Time = std::time::SystemTime;

    fn now(&self) -> Self::Time {
        std::time::UNIX_EPOCH
    }
}

async fn serve_controls(process: ScriptedProcessHarness) -> Result<Vec<String>, std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut operations = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if request["type"] == "user" {
            let content = request["message"]["content"].as_str().map_or(0, str::len);
            operations.push(format!("user:{content}"));
            continue;
        }
        let operation = request["request"]["subtype"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("missing control operation"))?;
        let response = match operation {
            "initialize" => json!({"claude_code_version":"scripted-cli"}),
            "mcp_status" => json!({"mcpServers":[{"name":"scripted","status":"connected"}]}),
            "mcp_set_servers" => json!({"added":["large"],"removed":[],"errors":{}}),
            _ => return Err(std::io::Error::other("unexpected control operation")),
        };
        operations.push(operation.to_owned());
        let mut frame = serde_json::to_vec(
            &json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":response}}),
        )?;
        frame.push(b'\n');
        stdout.write_all(&frame).await?;
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(operations)
}

/// CLI arguments must distinguish omission from explicit defaults and preserve list order.
#[tokio::test]
async fn optional_permission_and_settings_choices_reach_process_arguments() -> Result<(), Box<dyn std::error::Error>> {
    for (permission_mode, literal) in [
        (None, None),
        (Some(PermissionMode::Default), Some("default")),
        (Some(PermissionMode::AcceptEdits), Some("acceptEdits")),
        (Some(PermissionMode::Plan), Some("plan")),
        (Some(PermissionMode::BypassPermissions), Some("bypassPermissions")),
        (Some(PermissionMode::DontAsk), Some("dontAsk")),
        (Some(PermissionMode::Auto), Some("auto")),
    ] {
        for (setting_sources, sources) in [
            (None, None),
            (Some(vec![]), Some("")),
            (
                Some(vec![
                    SettingSource::Local,
                    SettingSource::User,
                    SettingSource::Project,
                    SettingSource::User,
                ]),
                Some("local,user,project,user"),
            ),
        ] {
            let (process, harness) = ScriptedProcessSpawner::new();
            let observer = process.observer();
            let server = tokio::spawn(serve_controls(harness));
            let client = ClaudeSdkClient::connect_with_adapters(
                ClaudeAgentOptions {
                    permission_mode: permission_mode.clone(),
                    setting_sources,
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
            .await?;
            client.disconnect().await?;
            assert_eq!(server.await??, ["initialize"]);
            let commands = observer.commands()?;
            assert_eq!(commands.len(), 1);
            let command = &commands[0];
            assert_eq!(command.executable, if cfg!(windows) { "claude.exe" } else { "claude" });
            for (prefix, expected) in [("--permission-mode=", literal), ("--setting-sources=", sources)] {
                let values = command
                    .arguments
                    .iter()
                    .filter_map(|arg| arg.to_str()?.strip_prefix(prefix))
                    .collect::<Vec<_>>();
                assert_eq!(values, expected.into_iter().collect::<Vec<_>>());
            }
        }
    }
    Ok(())
}

/// Configuration assembly and command construction must not invent model or path restrictions.
#[tokio::test]
async fn unrestricted_model_and_cli_path_reach_process_adapter() -> Result<(), Box<dyn std::error::Error>> {
    for (model, path) in [("", ""), ("model\nname", "tools/claude")] {
        let (process, harness) = ScriptedProcessSpawner::new();
        let observer = process.observer();
        let server = tokio::spawn(serve_controls(harness));
        let client = ClaudeSdkClient::connect_with_adapters(
            ClaudeAgentOptions {
                cli_path: Some(path.into()),
                model: Some(model.into()),
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
        .await?;
        client.disconnect().await?;
        assert_eq!(server.await??, ["initialize"]);
        let commands = observer.commands()?;
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].executable, path);
        assert!(
            commands[0]
                .arguments
                .contains(&OsString::from(format!("--model={model}")))
        );
    }
    Ok(())
}

#[tokio::test]
async fn injected_process_runs_protocol_and_closes_input_on_disconnect() -> Result<(), Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve_controls(harness));
    let directory = PathBuf::from("sdk-scripted-working-directory");
    let mut options = ClaudeAgentOptions {
        cli_path: Some("sdk-scripted-command".into()),
        cwd: Some(directory.clone()),
        env: trogon_claude_agent_sdk::Environment::from([("SDK_TEST_SENTINEL".into(), "child-value".into())]),
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(trogon_claude_agent_sdk::Timeout::new(std::time::Duration::from_secs(2)));
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(trogon_claude_agent_sdk::Timeout::new(
        std::time::Duration::from_secs(5),
    ));
    let adapters = RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-directory"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    };
    let client = ClaudeSdkClient::connect_with_adapters(options, adapters).await?;
    assert_eq!(
        client.connection_info().claude_code_version.as_deref(),
        Some("scripted-cli")
    );
    let status = client.handle().mcp_status().await?.decode()?;
    assert_eq!(status.mcp_servers[0]["name"], "scripted");
    client.disconnect().await?;
    assert_eq!(server.await??, ["initialize", "mcp_status"]);
    let commands = observer.commands()?;
    assert_eq!(commands.len(), 1);
    let command = &commands[0];
    assert_eq!(command.executable, "sdk-scripted-command");
    assert_eq!(command.directory, Some(directory));
    assert!(
        command
            .arguments
            .windows(2)
            .any(|pair| pair == ["--input-format", "stream-json"])
    );
    assert!(
        command
            .arguments
            .windows(2)
            .any(|pair| pair == ["--output-format", "stream-json"])
    );
    assert_eq!(command.environment.get(&OsString::from("CLAUDECODE")), Some(&None));
    assert_eq!(
        command.environment.get(&OsString::from("SDK_TEST_SENTINEL")),
        Some(&Some("child-value".into()))
    );
    assert_eq!(
        command
            .environment
            .get(&OsString::from("CLAUDE_CODE_SDK_READS_SESSION_STATE")),
        Some(&Some("1".into()))
    );
    assert!(observer.events()?.contains(&ScriptedProcessEvent::StdinClosed));
    Ok(())
}

#[tokio::test]
async fn injected_session_adapters_resolve_resume_and_clean_up() -> Result<(), Box<dyn std::error::Error>> {
    let filesystem = RecordingFileSystem::new()?;
    let project = filesystem.path("project");
    filesystem.create_dir_all(&project)?;
    let config = filesystem.path("config");
    let temporary = filesystem.path("tmp");
    filesystem.create_dir_all(&temporary)?;
    let session_id = "123e4567-e89b-42d3-a456-426614174000";
    let project_key = SessionProjectKey::for_directory_with_filesystem(&project, &filesystem)?;
    let relative_transcript = format!("projects/{}/{session_id}.jsonl", project_key.as_str());
    let store = Arc::new(InMemorySessionStore::with_clock(FrozenClock));
    let key = SessionKey::new(project_key, SessionId::from(session_id))?;
    let entry = SessionEntry::new(serde_json::from_value(json!({"type":"assistant","text":"seed"}))?);
    store.append(&key, std::slice::from_ref(&entry)).await?;
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve_controls(harness));
    let options = ClaudeAgentOptions {
        cli_path: Some("sdk-scripted-session-command".into()),
        resume: Some(key.session_id().clone()),
        session_store: Some(SessionStoreOptions::from(store)),
        ..ClaudeAgentOptions::default()
    };
    let client = ClaudeSdkClient::connect_with_adapters(
        options,
        RuntimeAdapters {
            environment: FixedEnvironment {
                directory: project.clone(),
                variables: BTreeMap::from([
                    ("CLAUDE_CONFIG_DIR".into(), config.clone().into_os_string()),
                    ("TMPDIR".into(), temporary.clone().into_os_string()),
                ]),
            },
            process,
            filesystem: filesystem.clone(),
        },
    )
    .await?;
    let materialized = std::fs::read_dir(&temporary)?
        .next()
        .ok_or("missing materialized configuration")??
        .path();
    let transcript = materialized.join(&relative_transcript);
    let mut expected = serde_json::to_vec(entry.as_object())?;
    expected.push(b'\n');
    assert_eq!(filesystem.read(&transcript)?, expected);
    assert!(!filesystem.exists(&config.join(&relative_transcript)));
    let commands = observer.commands()?;
    assert_eq!(commands.len(), 1);
    assert!(
        commands[0]
            .arguments
            .iter()
            .any(|arg| arg == std::ffi::OsStr::new(&format!("--resume={session_id}")))
    );
    assert_eq!(
        commands[0].environment.get(&OsString::from("CLAUDE_CONFIG_DIR")),
        Some(&Some(materialized.clone().into_os_string()))
    );
    client.disconnect().await?;
    assert_eq!(server.await??, ["initialize"]);
    assert!(!filesystem.exists(&materialized));
    assert!(observer.events()?.contains(&ScriptedProcessEvent::StdinClosed));
    Ok(())
}

#[tokio::test]
async fn missing_injected_config_stops_before_process_creation() -> Result<(), Box<dyn std::error::Error>> {
    let filesystem = RecordingFileSystem::new()?;
    let environment = TrackedEnvironment::new(&filesystem);
    let (process, _harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let options = ClaudeAgentOptions {
        session_store: Some(session_options()),
        ..ClaudeAgentOptions::default()
    };
    let result = ClaudeSdkClient::connect_with_adapters(
        options,
        RuntimeAdapters {
            environment: environment.clone(),
            process,
            filesystem: filesystem.clone(),
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(trogon_claude_agent_sdk::Error::Session(
            SessionError::MissingConfigDirectory
        ))
    ));
    assert!(observer.commands()?.is_empty());
    assert_eq!(
        environment.accesses()?,
        [
            EnvironmentAccess::CurrentDirectory,
            EnvironmentAccess::Variable("CLAUDE_CONFIG_DIR".into()),
            EnvironmentAccess::Variable("HOME".into()),
        ]
    );
    Ok(())
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn process_adapters<P>(process: P) -> RuntimeAdapters<FixedEnvironment, P> {
    RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-directory"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    }
}

#[tokio::test]
async fn injected_spawn_failure_preserves_its_typed_source_and_is_not_retried() -> TestResult {
    let (process, _harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    process.fail_spawn(io::ErrorKind::PermissionDenied)?;
    let result = ClaudeSdkClient::connect_with_adapters(ClaudeAgentOptions::default(), process_adapters(process)).await;
    assert!(
        matches!(result, Err(trogon_claude_agent_sdk::Error::Spawn(source)) if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert_eq!(observer.commands()?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn missing_process_pipes_return_typed_error_and_drop_the_spawned_child() -> TestResult {
    for missing in [MissingPipe::Stdin, MissingPipe::Stdout, MissingPipe::Stderr] {
        let (process, mut harness) = ScriptedProcessSpawner::new();
        let observer = process.observer();
        process.omit(missing)?;
        let result =
            ClaudeSdkClient::connect_with_adapters(ClaudeAgentOptions::default(), process_adapters(process)).await;
        assert!(
            matches!(result, Err(trogon_claude_agent_sdk::Error::PipeUnavailable)),
            "{missing:?}"
        );
        assert_eq!(observer.commands()?.len(), 1, "{missing:?}");
        observer.wait_for(ScriptedProcessEvent::StdinClosed).await?;
        let mut bytes = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(10),
            tokio::io::AsyncReadExt::read_to_end(&mut harness.stdin, &mut bytes),
        )
        .await??;
        assert!(
            bytes.is_empty(),
            "no initialization before all pipes exist: {missing:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn cancelling_connect_after_initialization_request_closes_input() -> TestResult {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let mut lines = BufReader::new(harness.stdin).lines();
    let connection = tokio::spawn(ClaudeSdkClient::connect_with_adapters(
        ClaudeAgentOptions::default(),
        process_adapters(process),
    ));
    let request = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await??
        .ok_or("missing initialization request")?;
    let request: Value = serde_json::from_str(&request)?;
    assert_eq!(request["request"]["subtype"], "initialize");
    connection.abort();
    assert!(connection.await.is_err_and(|error| error.is_cancelled()));
    assert!(
        tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await??
            .is_none()
    );
    drop(harness.stdout);
    drop(harness.stderr);
    harness.observer.complete(0);
    observer.wait_for(ScriptedProcessEvent::StdinClosed).await?;
    Ok(())
}

#[tokio::test]
async fn dropping_owner_shuts_down_even_when_command_handle_survives() -> TestResult {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve_controls(harness));
    let client =
        ClaudeSdkClient::connect_with_adapters(ClaudeAgentOptions::default(), process_adapters(process)).await?;
    let handle = client.handle();
    drop(client);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), server).await???,
        ["initialize"]
    );
    observer.wait_for(ScriptedProcessEvent::StdinClosed).await?;
    assert!(matches!(
        handle.mcp_status().await,
        Err(trogon_claude_agent_sdk::Error::NotReadyForWriting)
    ));
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EnvironmentAccess {
    Variable(OsString),
    CurrentDirectory,
}

#[derive(Clone)]
struct TrackedEnvironment {
    directory: Result<PathBuf, io::ErrorKind>,
    variables: BTreeMap<OsString, OsString>,
    accesses: Arc<Mutex<Vec<EnvironmentAccess>>>,
}

impl TrackedEnvironment {
    fn new(filesystem: &RecordingFileSystem) -> Self {
        Self {
            directory: Ok(filesystem.path("project")),
            variables: BTreeMap::new(),
            accesses: Arc::default(),
        }
    }

    /// Session-related accesses only: the SDK also reads these names to plan the child's own environment
    /// (entrypoint, version, debug and session-state markers, `PATH`, `HOME`), which is not session lookup.
    fn accesses(&self) -> io::Result<Vec<EnvironmentAccess>> {
        const LAUNCH_VARIABLES: [&str; 7] = [
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_AGENT_SDK_VERSION",
            "CLAUDE_CODE_SDK_READS_SESSION_STATE",
            "DEBUG_CLAUDE_AGENT_SDK",
            "CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK",
            "PATH",
            "USERPROFILE",
        ];
        let mut home_reads_skipped = true;
        Ok(self
            .accesses
            .lock()
            .map_err(|_| io::Error::other("environment tracking poisoned"))?
            .iter()
            .filter(|access| match access {
                EnvironmentAccess::Variable(key) => {
                    let key = key.to_string_lossy();
                    if LAUNCH_VARIABLES.contains(&key.as_ref()) {
                        return false;
                    }
                    // The launch capture reads HOME once, before any session lookup.
                    if key == "HOME" && home_reads_skipped {
                        home_reads_skipped = false;
                        return false;
                    }
                    true
                }
                _ => true,
            })
            .cloned()
            .collect())
    }

    fn record(&self, access: EnvironmentAccess) -> io::Result<()> {
        self.accesses
            .lock()
            .map_err(|_| io::Error::other("environment tracking poisoned"))?
            .push(access);
        Ok(())
    }
}

impl EnvironmentVariable for TrackedEnvironment {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.record(EnvironmentAccess::Variable(key.to_owned())).ok()?;
        self.variables.get(key).cloned()
    }
}

impl TemporaryDirectory for TrackedEnvironment {
    fn temp_dir(&self) -> PathBuf {
        self.variables
            .get(OsStr::new("TMPDIR"))
            .map_or_else(|| PathBuf::from("/unused-temporary"), PathBuf::from)
    }
}

impl CurrentDirectory for TrackedEnvironment {
    type Error = io::Error;

    fn current_dir(&self) -> Result<PathBuf, Self::Error> {
        self.record(EnvironmentAccess::CurrentDirectory)?;
        self.directory.clone().map_err(io::Error::from)
    }
}

fn session_options() -> SessionStoreOptions {
    SessionStoreOptions::from(Arc::new(InMemorySessionStore::with_clock(FrozenClock)))
}

async fn connect_with_tracked_environment(
    filesystem: &RecordingFileSystem,
    options: ClaudeAgentOptions,
    environment: TrackedEnvironment,
) -> Result<CapturedProcessCommand, Box<dyn std::error::Error>> {
    let filesystem = filesystem.clone();
    filesystem.create_dir_all(&filesystem.path("project"))?;
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(serve_controls(harness));
    let client = ClaudeSdkClient::connect_with_adapters(
        options,
        RuntimeAdapters {
            environment,
            process,
            filesystem,
        },
    )
    .await?;
    client.disconnect().await?;
    assert_eq!(server.await??, ["initialize"]);
    let commands = observer.commands()?;
    assert_eq!(commands.len(), 1);
    commands
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::other("missing captured process command").into())
}

#[tokio::test]
async fn explicit_store_config_and_working_directory_short_circuit_environment_lookups() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let environment = TrackedEnvironment {
        directory: Err(io::ErrorKind::PermissionDenied),
        variables: BTreeMap::from([
            ("CLAUDE_CONFIG_DIR".into(), "/unused-environment-config".into()),
            ("HOME".into(), "/unused-home".into()),
        ]),
        ..TrackedEnvironment::new(&filesystem)
    };
    let mut store = session_options();
    store.config_directory = Some(filesystem.path("explicit-store-config"));
    let command = connect_with_tracked_environment(
        &filesystem,
        ClaudeAgentOptions {
            cwd: Some(filesystem.path("project")),
            session_store: Some(store),
            ..ClaudeAgentOptions::default()
        },
        environment.clone(),
    )
    .await?;
    assert!(environment.accesses()?.is_empty());
    assert_eq!(command.directory, Some(filesystem.path("project")));
    assert_eq!(
        command.environment.get(OsStr::new("CLAUDE_CONFIG_DIR")),
        Some(&Some(filesystem.path("explicit-store-config").into_os_string()))
    );
    Ok(())
}

#[tokio::test]
async fn child_config_override_precedes_environment_and_accepts_matching_store_config() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let child_config = filesystem.path("child-config");
    let child_config_text = child_config.to_str().ok_or("temporary path is not UTF-8")?;
    for configured in [None, Some(child_config.clone())] {
        let environment = TrackedEnvironment {
            variables: BTreeMap::from([("CLAUDE_CONFIG_DIR".into(), "/unused-environment-config".into())]),
            ..TrackedEnvironment::new(&filesystem)
        };
        let mut store = session_options();
        store.config_directory = configured;
        let command = connect_with_tracked_environment(
            &filesystem,
            ClaudeAgentOptions {
                cwd: Some(filesystem.path("project")),
                env: trogon_claude_agent_sdk::Environment::from([(
                    "CLAUDE_CONFIG_DIR".into(),
                    child_config_text.into(),
                )]),
                session_store: Some(store),
                ..ClaudeAgentOptions::default()
            },
            environment.clone(),
        )
        .await?;
        assert!(environment.accesses()?.is_empty());
        assert_eq!(
            command.environment.get(OsStr::new("CLAUDE_CONFIG_DIR")),
            Some(&Some(child_config.clone().into_os_string()))
        );
    }
    Ok(())
}

#[tokio::test]
/// TypeScript only warns when the child `CLAUDE_CONFIG_DIR` differs from the store configuration; the child value
/// still reaches the subprocess.
async fn s21_conflicting_config_warns_and_keeps_the_child_value() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let environment = TrackedEnvironment::new(&filesystem);
    let mut store = session_options();
    store.config_directory = Some(filesystem.path("store-config"));
    let command = connect_with_tracked_environment(
        &filesystem,
        ClaudeAgentOptions {
            cwd: Some(filesystem.path("project")),
            env: trogon_claude_agent_sdk::Environment::from([("CLAUDE_CONFIG_DIR".into(), "/child-config".into())]),
            session_store: Some(store),
            ..ClaudeAgentOptions::default()
        },
        environment.clone(),
    )
    .await?;
    assert!(environment.accesses()?.is_empty());
    assert_eq!(
        command.environment.get(OsStr::new("CLAUDE_CONFIG_DIR")),
        Some(&Some(OsString::from("/child-config")))
    );
    Ok(())
}

#[tokio::test]
async fn injected_config_short_circuits_home_and_resolves_current_directory_once() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let config = filesystem.path("environment-config");
    let environment = TrackedEnvironment {
        variables: BTreeMap::from([
            ("CLAUDE_CONFIG_DIR".into(), config.clone().into_os_string()),
            ("HOME".into(), "/unused-home".into()),
        ]),
        ..TrackedEnvironment::new(&filesystem)
    };
    let command = connect_with_tracked_environment(
        &filesystem,
        ClaudeAgentOptions {
            session_store: Some(session_options()),
            ..ClaudeAgentOptions::default()
        },
        environment.clone(),
    )
    .await?;
    assert_eq!(
        environment.accesses()?,
        [
            EnvironmentAccess::CurrentDirectory,
            EnvironmentAccess::Variable("CLAUDE_CONFIG_DIR".into()),
        ]
    );
    assert_eq!(
        command.environment.get(OsStr::new("CLAUDE_CONFIG_DIR")),
        None,
        "the child inherits the caller configuration directory"
    );
    assert_eq!(
        command.directory,
        Some(filesystem.path("project")),
        "the child runs in the directory the session key was derived from"
    );
    Ok(())
}

#[tokio::test]
async fn missing_config_variable_falls_back_to_injected_home_in_order() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let home = filesystem.path("home");
    let environment = TrackedEnvironment {
        variables: BTreeMap::from([("HOME".into(), home.clone().into_os_string())]),
        ..TrackedEnvironment::new(&filesystem)
    };
    let command = connect_with_tracked_environment(
        &filesystem,
        ClaudeAgentOptions {
            session_store: Some(session_options()),
            ..ClaudeAgentOptions::default()
        },
        environment.clone(),
    )
    .await?;
    assert_eq!(
        environment.accesses()?,
        [
            EnvironmentAccess::CurrentDirectory,
            EnvironmentAccess::Variable("CLAUDE_CONFIG_DIR".into()),
            EnvironmentAccess::Variable("HOME".into()),
        ]
    );
    assert_eq!(
        command.environment.get(OsStr::new("CLAUDE_CONFIG_DIR")),
        None,
        "the child inherits the default configuration directory"
    );
    assert_eq!(command.directory, Some(filesystem.path("project")));
    Ok(())
}

#[tokio::test]
async fn explicit_working_directory_skips_failing_current_directory_provider() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let environment = TrackedEnvironment {
        directory: Err(io::ErrorKind::PermissionDenied),
        variables: BTreeMap::from([(
            "CLAUDE_CONFIG_DIR".into(),
            filesystem.path("environment-config").into_os_string(),
        )]),
        ..TrackedEnvironment::new(&filesystem)
    };
    let command = connect_with_tracked_environment(
        &filesystem,
        ClaudeAgentOptions {
            cwd: Some(filesystem.path("project")),
            session_store: Some(session_options()),
            ..ClaudeAgentOptions::default()
        },
        environment.clone(),
    )
    .await?;
    assert_eq!(
        environment.accesses()?,
        [EnvironmentAccess::Variable("CLAUDE_CONFIG_DIR".into())]
    );
    assert_eq!(command.directory, Some(filesystem.path("project")));
    Ok(())
}

#[tokio::test]
async fn current_directory_failure_stops_before_config_lookup_or_process_creation() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let environment = TrackedEnvironment {
        directory: Err(io::ErrorKind::PermissionDenied),
        ..TrackedEnvironment::new(&filesystem)
    };
    let (process, _harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let result = ClaudeSdkClient::connect_with_adapters(
        ClaudeAgentOptions {
            session_store: Some(session_options()),
            ..ClaudeAgentOptions::default()
        },
        RuntimeAdapters {
            environment: environment.clone(),
            process,
            filesystem: filesystem.clone(),
        },
    )
    .await;
    assert!(
        matches!(result, Err(trogon_claude_agent_sdk::Error::Io(source)) if source.kind() == io::ErrorKind::PermissionDenied)
    );
    assert_eq!(environment.accesses()?, [EnvironmentAccess::CurrentDirectory]);
    assert!(observer.commands()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn ordinary_connection_never_reads_session_environment_or_current_directory() -> TestResult {
    let filesystem = RecordingFileSystem::new()?;
    let environment = TrackedEnvironment {
        directory: Err(io::ErrorKind::PermissionDenied),
        ..TrackedEnvironment::new(&filesystem)
    };
    connect_with_tracked_environment(&filesystem, ClaudeAgentOptions::default(), environment.clone()).await?;
    assert!(environment.accesses()?.is_empty());
    Ok(())
}

/// Both upstream SDKs write outbound frames uncapped; only inbound frames are bounded.
#[tokio::test]
async fn s12_outbound_prompts_and_controls_are_never_capped_by_the_inbound_frame_limit() -> TestResult {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve_controls(harness));
    let mut options = ClaudeAgentOptions::default();
    options.limits.max_buffer_size = Some(std::num::NonZeroUsize::new(1024).ok_or("invalid limit")?);
    let client = ClaudeSdkClient::connect_with_adapters(options, process_adapters(process)).await?;
    client.send_prompt(Prompt::from("x".repeat(2048))).await?;
    let replacement = McpServers::from([(
        McpServerName::from("large"),
        McpServerOptions::Http(McpRemoteServerOptions::new(format!(
            "https://example.com/{}",
            "x".repeat(2048)
        ))),
    )]);
    let result = client
        .handle()
        .set_mcp_servers(replacement, SdkMcpServers::new())
        .await?;
    assert_eq!(result.added, ["large"]);
    client.disconnect().await?;
    assert_eq!(server.await??, ["initialize", "user:2048", "mcp_set_servers"]);
    Ok(())
}
