#![cfg(all(unix, feature = "test-support"))]

//! The `--settings` value the Python dialect builds where `ClaudeSDKClient.connect` and `query()` build it, against
//! what CPython 3.12 and the pinned `claude_agent_sdk` pass there. `json.loads` reads less deeply in `query()` than
//! in `ClaudeSDKClient.connect`, because more of CPython's C recursion checks are already open there. A settings file
//! the `utf-8-sig` codec cannot decode raises the same `CLIConnectionError` from both, before the CLI starts.

use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, Error, Prompt, SandboxSettings, SettingsInput,
    SettingsLoadCauseError,
    adapters::{NativeFileSystem, RuntimeAdapters, SpawnProcess},
    query_with_adapters,
    test_support::FixedEnvironment,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/settings_option/python_settings.json");

/// Records the arguments of every command it is asked to start, and starts none.
#[derive(Clone, Default)]
struct RecordingSpawner {
    commands: Arc<Mutex<Vec<Vec<String>>>>,
}

impl RecordingSpawner {
    /// How many commands it was asked to start.
    fn started(&self) -> Result<usize, Box<dyn std::error::Error>> {
        Ok(self
            .commands
            .lock()
            .map_err(|_| "the command recording is poisoned")?
            .len())
    }

    /// The SHA-256 and length of each `--settings` value passed so far.
    fn settings_values(&self) -> Result<Vec<(String, u64)>, Box<dyn std::error::Error>> {
        let commands = self.commands.lock().map_err(|_| "the command recording is poisoned")?;
        let passed = commands.iter().filter_map(|arguments| {
            let position = arguments.iter().position(|argument| argument == "--settings")?;
            arguments.get(position + 1)
        });
        let mut values = Vec::new();
        for value in passed {
            let digest: String = Sha256::digest(value.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            values.push((digest, u64::try_from(value.len())?));
        }
        Ok(values)
    }
}

impl SpawnProcess for RecordingSpawner {
    type Error = io::Error;

    fn spawn(&self, command: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
        let arguments = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned());
        self.commands
            .lock()
            .map_err(|_| io::Error::other("the command recording is poisoned"))?
            .push(arguments.collect());
        Err(io::Error::from(io::ErrorKind::NotFound))
    }
}

/// One `depth` entry of the goldens: the deepest nesting `json.loads` reads for that caller, the `--settings` value
/// there, and the `RecursionError` message one level deeper.
struct Depth {
    loads: usize,
    value: (String, u64),
    error: String,
}

fn depth(caller: &str) -> Result<Depth, Box<dyn std::error::Error>> {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entry = goldens["depth"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["caller"] == caller))
        .ok_or("the goldens have no entry for the caller")?;
    let field = |name: &str| {
        entry[name]
            .as_str()
            .map(str::to_owned)
            .ok_or("a depth entry misses a field")
    };
    Ok(Depth {
        loads: usize::try_from(entry["loads"].as_u64().ok_or("a depth entry has no loads")?)?,
        value: (
            field("value_sha256")?,
            entry["value_length"].as_u64().ok_or("a depth entry has no length")?,
        ),
        error: field("error")?,
    })
}

fn options(depth: usize) -> Result<ClaudeAgentOptions, serde_json::Error> {
    Ok(ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        cli_path: Some("claude".into()),
        settings: Some(SettingsInput::from_wire_string(format!(
            "{}null{}",
            "{\"a\":".repeat(depth),
            "}".repeat(depth)
        ))),
        sandbox: Some(serde_json::from_str::<SandboxSettings>(r#"{"enabled":true}"#)?),
        ..ClaudeAgentOptions::default()
    })
}

fn adapters(process: RecordingSpawner) -> RuntimeAdapters<FixedEnvironment, RecordingSpawner, NativeFileSystem> {
    RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-directory"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    }
}

/// Builds the command at the golden depth and one level deeper, and returns the `--settings` values passed at the
/// golden depth, the error one level deeper, and the `--settings` values passed one level deeper.
async fn attempt<C>(
    caller: &Depth,
    connect: C,
) -> Result<(Vec<(String, u64)>, Option<String>, usize), Box<dyn std::error::Error>>
where
    C: AsyncFn(ClaudeAgentOptions, RecordingSpawner) -> Option<Error>,
{
    let read = RecordingSpawner::default();
    connect(options(caller.loads)?, read.clone()).await;
    let refused = RecordingSpawner::default();
    let error = connect(options(caller.loads + 1)?, refused.clone()).await;
    Ok((
        read.settings_values()?,
        error.map(|error| error.to_string()),
        refused.settings_values()?.len(),
    ))
}

#[tokio::test]
async fn client_connect_reads_settings_as_deep_as_claude_sdk_client_connect() -> TestResult {
    let caller = depth("client_connect")?;
    let actual = attempt(&caller, async |options, process| {
        ClaudeSdkClient::connect_with_adapters(options, adapters(process))
            .await
            .err()
    })
    .await?;
    assert_eq!(actual, (vec![caller.value], Some(caller.error), 0));
    Ok(())
}

#[tokio::test]
async fn query_reads_settings_as_deep_as_python_query() -> TestResult {
    let caller = depth("query")?;
    let actual = attempt(&caller, async |options, process| {
        query_with_adapters(Prompt::from("hi"), options, adapters(process))
            .await
            .err()
    })
    .await?;
    assert_eq!(actual, (vec![caller.value], Some(caller.error), 0));
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(text: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2).unwrap_or_default(), 16))
        .collect()
}

/// What a caller raised for a settings file, as the goldens record it: the class name, whether it is a
/// `ClaudeSDKError`, the message, and the class name, `encoding`, `object` as hex, `start`, `end` and `reason` of
/// its `__cause__`, followed by how many commands were started.
fn raised(error: Option<Error>, started: usize) -> Value {
    let error = match error {
        Some(Error::SettingsLoad(error)) => error,
        other => return Value::String(format!("not a settings load error: {other:?}")),
    };
    let SettingsLoadCauseError::UnicodeDecode(cause) = error.cause() else {
        return Value::String(format!("not a decode error: {:?}", error.cause()));
    };
    serde_json::json!([
        "CLIConnectionError",
        true,
        error.to_string(),
        "UnicodeDecodeError",
        cause.encoding(),
        hex(cause.object()),
        cause.start(),
        cause.end(),
        cause.reason().as_str(),
        started
    ])
}

/// Each `raised` entry of the goldens is a settings file the `utf-8-sig` codec cannot decode, with what
/// `_build_settings_value`, `ClaudeSDKClient.connect` and `query()` raise for it. Each raises the same
/// `CLIConnectionError`, whose `__cause__` is the `UnicodeDecodeError`, and `connect` and `query()` raise it before
/// they start the CLI.
#[tokio::test]
async fn an_undecodable_settings_file_raises_what_connect_and_query_raise() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entries = goldens["raised"]
        .as_array()
        .ok_or("the goldens have no undecodable files")?;
    let folder = tempfile::tempdir()?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let path = folder.path().join(format!("settings-{index}.json"));
        std::fs::write(
            &path,
            from_hex(entry[0].as_str().ok_or("an undecodable file has no bytes")?)?,
        )?;
        let callers = entry[1].as_array().ok_or("an undecodable file has no callers")?;
        for caller in callers {
            let mut found = caller.as_array().ok_or("a caller is not a list")?.clone();
            let name = found.remove(0);
            found.push(Value::from(0));
            expected.push((name, Value::Array(found)));
        }
        let options = ClaudeAgentOptions {
            argv_style: ArgvStyle::Python,
            cli_path: Some("claude".into()),
            settings: Some(SettingsInput::from_wire_string(path.to_string_lossy().into_owned())),
            sandbox: Some(serde_json::from_str::<SandboxSettings>(r#"{"enabled":true}"#)?),
            ..ClaudeAgentOptions::default()
        };
        let connecting = RecordingSpawner::default();
        let connected = ClaudeSdkClient::connect_with_adapters(options.clone(), adapters(connecting.clone()))
            .await
            .err();
        let querying = RecordingSpawner::default();
        let queried = query_with_adapters(Prompt::from("hi"), options, adapters(querying.clone()))
            .await
            .err();
        // `_build_settings_value` has no public entry of its own; the goldens show it raises what `connect` raises.
        let connect_raised = raised(connected, connecting.started()?);
        actual.push((Value::from("module"), connect_raised.clone()));
        actual.push((Value::from("client_connect"), connect_raised));
        actual.push((Value::from("query"), raised(queried, querying.started()?)));
    }
    assert_eq!((entries.len(), actual), (5, expected));
    Ok(())
}
