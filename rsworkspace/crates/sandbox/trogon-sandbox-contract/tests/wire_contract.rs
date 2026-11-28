//! Golden files that pin the serialized shape of the public command, log and terminal messages.
//!
//! Each case builds a value, serializes it and compares the JSON with a file under `tests/wire_fixtures/`.
//! A renamed field, a removed variant, a changed tag or a new always-serialized field changes the JSON and
//! fails the test. Every case also reads its own JSON back into the type, so the value must survive.
//!
//! To change the contract on purpose, regenerate the files and review the diff in the same change:
//!
//! ```text
//! UPDATE_WIRE_FIXTURES=1 cargo test -p trogon-sandbox-contract --test wire_contract
//! ```

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use trogon_sandbox_contract::{
    Base64Data, CommandExitMarker, CommandExitReport, CommandOutput, CommandOutputChunk, CommandTimeoutStatus,
    ExecutionId, KillCommandRequest, KillCommandResponse, LeaseError, LeaseId, LogChunk, LogsContinuation,
    OutputStream, OutputTruncation, PollCommandRequest, PollCommandResponse, ReadLogsRequest, ReadLogsResponse,
    StdinChunk, StdinEof, StdinSeq, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalInput,
    TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse, UnixSignal, WriteStdinRequest,
};
use trogon_std::env::{ReadEnv, SystemEnv};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The named cases of one fixture file, kept in name order so the file diffs cleanly.
struct Fixture {
    file: &'static str,
    cases: BTreeMap<String, Value>,
}

impl Fixture {
    fn new(file: &'static str) -> Self {
        Self {
            file,
            cases: BTreeMap::new(),
        }
    }

    fn case<T>(&mut self, name: &str, value: T) -> TestResult
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        let json = serde_json::to_value(&value)?;
        let back: T = serde_json::from_value(json.clone())?;
        assert_eq!(back, value, "case {name} does not survive a round trip");
        assert!(
            self.cases.insert(name.to_string(), json).is_none(),
            "duplicate case {name}"
        );
        Ok(())
    }

    fn check(self) -> TestResult {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/wire_fixtures")
            .join(self.file);
        let actual = serde_json::to_string_pretty(&self.cases)? + "\n";
        if SystemEnv.var_os("UPDATE_WIRE_FIXTURES").is_some() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, &actual)?;
            return Ok(());
        }
        let frozen = std::fs::read_to_string(&path).map_err(|error| {
            format!(
                "missing fixture {}: {error}; run with UPDATE_WIRE_FIXTURES=1 to create it",
                path.display()
            )
        })?;
        assert_eq!(
            serde_json::from_str::<Value>(&actual)?,
            serde_json::from_str::<Value>(&frozen)?,
            "the wire format of {} changed. If that is intended and backward compatible, regenerate with \
             UPDATE_WIRE_FIXTURES=1 and review the diff; otherwise revert.",
            self.file
        );
        Ok(())
    }
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn exec() -> ExecutionId {
    ExecutionId::new("exec-1")
}

fn terminal_request(op: TerminalOp) -> Result<TerminalRequest, Box<dyn std::error::Error>> {
    Ok(TerminalRequest {
        lease_id: lease(),
        terminal_id: TerminalId::new("term-1")?,
        op,
    })
}

#[test]
fn command_streaming_types_are_frozen() -> TestResult {
    let mut f = Fixture::new("command_streaming.json");
    f.case(
        "command_output_chunk",
        CommandOutputChunk {
            execution_id: exec(),
            stream: OutputStream::Stderr,
            seq: 7,
            data: b"hi".to_vec(),
        },
    )?;
    f.case(
        "command_exit_marker",
        CommandExitMarker {
            execution_id: exec(),
            exit_code: Some(3),
        },
    )?;
    f.case(
        "command_exit_marker_without_code",
        CommandExitMarker {
            execution_id: exec(),
            exit_code: None,
        },
    )?;
    f.case(
        "poll_command_request",
        PollCommandRequest {
            lease_id: lease(),
            execution_id: exec(),
        },
    )?;
    f.case("poll_command_running", PollCommandResponse::Running)?;
    f.case(
        "poll_command_completed",
        PollCommandResponse::Completed(CommandOutput {
            exit_code: 0,
            stdout: b"out".to_vec(),
            stderr: Vec::new(),
            timed_out: CommandTimeoutStatus::WithinDeadline,
            truncated: OutputTruncation::Truncated,
        }),
    )?;
    f.case("poll_command_not_found", PollCommandResponse::NotFound)?;
    f.case(
        "poll_command_denied",
        PollCommandResponse::Denied(LeaseError::NoProviderAvailable),
    )?;
    f.case(
        "kill_command_request",
        KillCommandRequest {
            lease_id: lease(),
            execution_id: exec(),
            signal: UnixSignal::Kill,
        },
    )?;
    f.case("kill_command_signaled", KillCommandResponse::Signaled)?;
    f.case("kill_command_already_completed", KillCommandResponse::AlreadyCompleted)?;
    f.case("kill_command_not_found", KillCommandResponse::NotFound)?;
    f.case(
        "write_stdin_request",
        WriteStdinRequest::new(
            lease(),
            exec(),
            StdinSeq::new(0),
            StdinChunk::new(b"x".to_vec())?,
            StdinEof::Close,
        )?,
    )?;
    f.check()
}

#[test]
fn log_reading_types_are_frozen() -> TestResult {
    let mut f = Fixture::new("read_logs.json");
    f.case(
        "read_logs_request",
        ReadLogsRequest {
            lease_id: lease(),
            execution_id: exec(),
            after_seq: 12,
            max_bytes: Some(4096),
            wait_ms: Some(1000),
        },
    )?;
    f.case(
        "read_logs_request_defaults",
        ReadLogsRequest {
            lease_id: lease(),
            execution_id: exec(),
            after_seq: 0,
            max_bytes: None,
            wait_ms: None,
        },
    )?;
    f.case(
        "read_logs_logs",
        ReadLogsResponse::Logs {
            chunks: vec![LogChunk {
                log_seq: 5,
                stream: OutputStream::Stdout,
                seq: 0,
                data: b"a".to_vec(),
            }],
            next_seq: 5,
            has_more: LogsContinuation::HasMore,
            truncated: OutputTruncation::Truncated,
            exited: CommandExitReport::Reported,
        },
    )?;
    f.case(
        "read_logs_logs_complete",
        ReadLogsResponse::Logs {
            chunks: Vec::new(),
            next_seq: 0,
            has_more: LogsContinuation::Complete,
            truncated: OutputTruncation::Complete,
            exited: CommandExitReport::NotReported,
        },
    )?;
    f.case("read_logs_unsupported", ReadLogsResponse::Unsupported)?;
    f.case(
        "read_logs_denied",
        ReadLogsResponse::Denied(LeaseError::NoProviderAvailable),
    )?;
    f.check()
}

#[test]
fn terminal_types_are_frozen() -> TestResult {
    let size = |value| TerminalDimension::new(value);
    let mut f = Fixture::new("terminal.json");
    f.case(
        "open",
        terminal_request(TerminalOp::Open {
            cols: size(80)?,
            rows: size(24)?,
            cmd: vec![TerminalArgument::new("bash")?, TerminalArgument::new("-l")?],
            env: BTreeMap::from([("TERM".to_string(), "xterm".to_string())]),
            cwd: Some("/work".to_string()),
            resume_from: None,
        })?,
    )?;
    f.case(
        "open_defaults",
        terminal_request(TerminalOp::Open {
            cols: size(80)?,
            rows: size(24)?,
            cmd: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            resume_from: None,
        })?,
    )?;
    f.case(
        "open_resume",
        terminal_request(TerminalOp::Open {
            cols: size(80)?,
            rows: size(24)?,
            cmd: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            resume_from: Some(41),
        })?,
    )?;
    f.case(
        "input",
        terminal_request(TerminalOp::Input {
            data: TerminalInput::new("bHMK")?,
        })?,
    )?;
    f.case(
        "resize",
        terminal_request(TerminalOp::Resize {
            cols: size(100)?,
            rows: size(40)?,
        })?,
    )?;
    f.case("close", terminal_request(TerminalOp::Close)?)?;
    f.case(
        "response_opened",
        TerminalResponse::Opened {
            terminal_id: TerminalId::new("term-1")?,
        },
    )?;
    f.case("response_done", TerminalResponse::Done)?;
    f.case("response_unsupported", TerminalResponse::Unsupported)?;
    f.case(
        "response_failed_too_many",
        TerminalResponse::Failed(TerminalError::TooManyTerminals { max: 8 }),
    )?;
    f.case(
        "output_data",
        TerminalOutput::Data {
            seq: 3,
            data: Base64Data::new("bHMK")?,
        },
    )?;
    f.case(
        "output_exit",
        TerminalOutput::Exit {
            seq: 4,
            exit_code: Some(0),
        },
    )?;
    f.case("output_resync", TerminalOutput::Resync { from_seq: 7 })?;
    f.check()
}
