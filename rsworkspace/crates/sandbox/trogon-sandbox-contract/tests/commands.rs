use std::collections::BTreeMap;
use std::num::NonZeroU16;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CommandDeadline, CommandDetachment, CommandEnv, CommandEnvError, CommandExecution, CommandExitMarker,
    CommandLaunchOrigin, CommandOutput, CommandOutputChunk, CommandTimeoutStatus, ExecutionId, LaunchCancellation,
    LaunchKey, LeaseError, LeaseId, ListProcessesRequest, ListProcessesResponse, OutputRetention, OutputStream,
    OutputTruncation, PollCommandRequest, PollCommandResponse, ProcessInfo, ProcessName, ProcessNameError, ProcessSpec,
    ProcessState, ReadinessNeedle, ReadinessNeedleError, ReadinessProbe, ReadyPorts, ReadyPortsError, RedactValue,
    RedactValues, RedactValuesError, RunCommandRequest, RunCommandRequestError, RunCommandResponse,
    RunCommandStartedResponse, SandboxUserName, StdinMode, StreamedOutput, WaitReadyRequest, WaitReadyResponse,
    WaitSandboxReadyRequest, WaitSandboxReadyResponse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn roundtrip<T>(value: &T, expected: Value) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value)?, expected);
    assert_eq!(&serde_json::from_value::<T>(expected)?, value);
    Ok(())
}

fn rejected<T: DeserializeOwned>(value: Value) -> TestResult {
    let error = serde_json::from_value::<T>(value)
        .err()
        .ok_or("the value must be rejected")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn exec() -> ExecutionId {
    ExecutionId::new("exec-1")
}

fn minimal_request() -> RunCommandRequest {
    RunCommandRequest {
        lease_id: lease(),
        command: vec!["true".to_string()],
        cwd: None,
        env: CommandEnv::default(),
        timeout_seconds: 60,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: OutputRetention::Buffer,
        max_output_bytes: None,
        redact_values: RedactValues::default(),
        launch_key: None,
        require_launch_cancellation: LaunchCancellation::NotRequired,
        execution: CommandExecution::Attached,
    }
}

fn minimal_json() -> Value {
    json!({
        "lease_id": "lease-1",
        "command": ["true"],
        "env": {},
        "timeout_seconds": 60,
        "detached": false,
        "stream_output": false,
        "stdin": "closed",
        "output_retention": "buffer",
        "deadline": "timeout",
    })
}

#[test]
fn a_minimal_request_reads_every_default() -> TestResult {
    let parsed: RunCommandRequest = serde_json::from_value(json!({"lease_id": "lease-1", "command": ["true"]}))?;
    assert_eq!(parsed, minimal_request());
    assert_eq!(RunCommandRequest::DEFAULT_TIMEOUT_SECONDS, 60);
    assert_eq!(CommandExecution::default(), CommandExecution::Attached);
    roundtrip(&minimal_request(), minimal_json())
}

#[test]
fn a_full_attached_request_round_trips() -> TestResult {
    let request = RunCommandRequest {
        cwd: Some("/work".to_string()),
        env: CommandEnv::new(BTreeMap::from([("A".to_string(), "b".to_string())]))?,
        timeout_seconds: 5,
        stream_output: StreamedOutput::Enabled,
        run_as_user: Some(SandboxUserName::new("root")?),
        output_retention: OutputRetention::StreamOnly,
        max_output_bytes: Some(1024),
        redact_values: RedactValues::new(vec![RedactValue::new("hunter2-literal")])?,
        launch_key: Some(LaunchKey::new("job-1")?),
        require_launch_cancellation: LaunchCancellation::Required,
        ..minimal_request()
    };
    roundtrip(
        &request,
        json!({
            "lease_id": "lease-1",
            "command": ["true"],
            "cwd": "/work",
            "env": {"A": "b"},
            "timeout_seconds": 5,
            "detached": false,
            "stream_output": true,
            "run_as_user": "root",
            "stdin": "closed",
            "output_retention": "stream_only",
            "deadline": "timeout",
            "max_output_bytes": 1024,
            "redact_values": ["hunter2-literal"],
            "launch_key": "job-1",
            "require_launch_cancellation": true,
        }),
    )
}

#[test]
fn a_detached_request_carries_its_options() -> TestResult {
    let request = RunCommandRequest {
        execution: CommandExecution::Detached {
            stdin: StdinMode::Interactive,
            deadline: CommandDeadline::Lease,
            process: Some(ProcessSpec {
                name: Some(ProcessName::new("web")?),
                readiness: Some(ReadinessProbe::TcpPort {
                    port: NonZeroU16::new(8080).ok_or("nonzero")?,
                }),
            }),
        },
        ..minimal_request()
    };
    let mut expected = minimal_json();
    expected["detached"] = json!(true);
    expected["stdin"] = json!("interactive");
    expected["deadline"] = json!("lease");
    expected["process"] = json!({"name": "web", "readiness": {"kind": "tcp_port", "port": 8080}});
    roundtrip(&request, expected)?;

    let plain = RunCommandRequest {
        execution: CommandExecution::Detached {
            stdin: StdinMode::Closed,
            deadline: CommandDeadline::Timeout,
            process: None,
        },
        ..minimal_request()
    };
    let mut expected = minimal_json();
    expected["detached"] = json!(true);
    roundtrip(&plain, expected)
}

#[test]
fn an_attached_request_cannot_ask_for_detached_options() -> TestResult {
    for (field, value) in [
        ("stdin", json!("interactive")),
        ("deadline", json!("lease")),
        ("process", json!({"name": "web"})),
    ] {
        let mut body = minimal_json();
        body[field] = value;
        rejected::<RunCommandRequest>(body)?;
    }
    Ok(())
}

#[test]
fn the_error_for_each_detached_option_is_distinct() -> TestResult {
    let mut interactive = minimal_json();
    interactive["stdin"] = json!("interactive");
    let mut lease_deadline = minimal_json();
    lease_deadline["deadline"] = json!("lease");
    let mut process = minimal_json();
    process["process"] = json!({});
    for (body, expected) in [
        (interactive, RunCommandRequestError::InteractiveStdinRequiresDetached),
        (lease_deadline, RunCommandRequestError::LeaseDeadlineRequiresDetached),
        (process, RunCommandRequestError::ProcessRequiresDetached),
    ] {
        let error = serde_json::from_value::<RunCommandRequest>(body)
            .err()
            .ok_or("the request must be rejected")?;
        assert_eq!(error.to_string(), expected.to_string());
    }
    Ok(())
}

#[test]
fn run_command_request_error_messages() {
    for (error, message) in [
        (
            RunCommandRequestError::InteractiveStdinRequiresDetached,
            "interactive stdin requires a detached command",
        ),
        (
            RunCommandRequestError::LeaseDeadlineRequiresDetached,
            "a lease deadline requires a detached command",
        ),
        (
            RunCommandRequestError::ProcessRequiresDetached,
            "a process spec requires a detached command",
        ),
    ] {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn a_request_rejects_bad_launch_keys_and_users() -> TestResult {
    let mut key = minimal_json();
    key["launch_key"] = json!("bad key");
    rejected::<RunCommandRequest>(key)?;
    let mut user = minimal_json();
    user["run_as_user"] = json!("Bad User");
    rejected::<RunCommandRequest>(user)
}

#[test]
fn redact_values_never_show_in_debug() -> TestResult {
    let value = RedactValue::new("hunter2-literal");
    assert_eq!(value.expose_secret(), "hunter2-literal");
    assert_eq!(format!("{value:?}"), "RedactValue([REDACTED])");
    assert_eq!(RedactValue::from("a".to_string()), RedactValue::new("a"));
    assert_eq!(String::from(RedactValue::new("a")), "a");
    roundtrip(&value, json!("hunter2-literal"))?;
    let request = RunCommandRequest {
        redact_values: RedactValues::new(vec![value])?,
        ..minimal_request()
    };
    assert!(format!("{request:?}").contains("[REDACTED]"));
    assert!(!format!("{request:?}").contains("hunter2"));
    Ok(())
}

#[test]
fn bool_flags_use_json_booleans() -> TestResult {
    assert!(CommandDetachment::Detached.is_detached());
    assert!(!CommandDetachment::default().is_detached());
    assert!(StreamedOutput::Enabled.is_enabled());
    assert!(!StreamedOutput::default().is_enabled());
    assert!(LaunchCancellation::Required.is_required());
    assert!(LaunchCancellation::default().is_not_required());
    assert!(!LaunchCancellation::Required.is_not_required());
    assert!(CommandTimeoutStatus::TimedOut.is_timed_out());
    assert!(CommandTimeoutStatus::default().is_within_deadline());
    assert!(!CommandTimeoutStatus::TimedOut.is_within_deadline());
    assert!(OutputTruncation::Truncated.is_truncated());
    assert!(OutputTruncation::default().is_complete());
    assert!(!OutputTruncation::Truncated.is_complete());
    roundtrip(&CommandDetachment::Attached, json!(false))?;
    roundtrip(&CommandDetachment::Detached, json!(true))?;
    roundtrip(&StreamedOutput::Disabled, json!(false))?;
    roundtrip(&StreamedOutput::Enabled, json!(true))?;
    roundtrip(&LaunchCancellation::NotRequired, json!(false))?;
    roundtrip(&LaunchCancellation::Required, json!(true))?;
    roundtrip(&CommandTimeoutStatus::WithinDeadline, json!(false))?;
    roundtrip(&CommandTimeoutStatus::TimedOut, json!(true))?;
    roundtrip(&OutputTruncation::Complete, json!(false))?;
    roundtrip(&OutputTruncation::Truncated, json!(true))
}

#[test]
fn command_deadline_uses_snake_case() -> TestResult {
    assert_eq!(CommandDeadline::default(), CommandDeadline::Timeout);
    roundtrip(&CommandDeadline::Timeout, json!("timeout"))?;
    roundtrip(&CommandDeadline::Lease, json!("lease"))
}

#[test]
fn command_output_from_an_older_peer_parses_with_flags_cleared() -> TestResult {
    let output: CommandOutput = serde_json::from_value(json!({"exit_code": 0, "stdout": [104], "stderr": []}))?;
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(output.truncated, OutputTruncation::Complete);
    assert_eq!(
        serde_json::to_value(&output)?,
        json!({"exit_code": 0, "stdout": [104], "stderr": []})
    );
    Ok(())
}

#[test]
fn command_output_flags_round_trip_when_set() -> TestResult {
    assert_eq!(CommandOutput::TIMED_OUT_EXIT_CODE, -1);
    let output = CommandOutput {
        exit_code: CommandOutput::TIMED_OUT_EXIT_CODE,
        timed_out: CommandTimeoutStatus::TimedOut,
        truncated: OutputTruncation::Truncated,
        ..Default::default()
    };
    roundtrip(
        &output,
        json!({"exit_code": -1, "stdout": [], "stderr": [], "timed_out": true, "truncated": true}),
    )
}

#[test]
fn streamed_output_markers() -> TestResult {
    roundtrip(
        &CommandOutputChunk {
            execution_id: exec(),
            stream: OutputStream::Stdout,
            seq: 2,
            data: vec![1],
        },
        json!({"execution_id": "exec-1", "stream": "stdout", "seq": 2, "data": [1]}),
    )?;
    roundtrip(
        &CommandExitMarker {
            execution_id: exec(),
            exit_code: None,
        },
        json!({"execution_id": "exec-1"}),
    )?;
    roundtrip(
        &CommandExitMarker {
            execution_id: exec(),
            exit_code: Some(3),
        },
        json!({"execution_id": "exec-1", "exit_code": 3}),
    )
}

#[test]
fn started_and_completed_responses() -> TestResult {
    assert_eq!(CommandLaunchOrigin::default(), CommandLaunchOrigin::Unknown);
    roundtrip(
        &RunCommandStartedResponse {
            execution_id: exec(),
            origin: CommandLaunchOrigin::Unknown,
        },
        json!({"execution_id": "exec-1"}),
    )?;
    for (origin, wire) in [
        (CommandLaunchOrigin::Created, "created"),
        (CommandLaunchOrigin::Reused, "reused"),
    ] {
        roundtrip(
            &RunCommandStartedResponse {
                execution_id: exec(),
                origin,
            },
            json!({"execution_id": "exec-1", "origin": wire}),
        )?;
    }
    let started = RunCommandStartedResponse {
        execution_id: exec(),
        origin: CommandLaunchOrigin::Unknown,
    };
    roundtrip(
        &RunCommandResponse::Started(started),
        json!({"outcome": "started", "execution_id": "exec-1"}),
    )?;
    roundtrip(
        &RunCommandResponse::Completed(CommandOutput::default()),
        json!({"outcome": "completed", "exit_code": 0, "stdout": [], "stderr": []}),
    )?;
    roundtrip(
        &RunCommandResponse::Denied(LeaseError::NoProviderAvailable),
        json!({"outcome": "denied", "kind": "no_provider_available"}),
    )
}

#[test]
fn polling() -> TestResult {
    roundtrip(
        &PollCommandRequest {
            lease_id: lease(),
            execution_id: exec(),
        },
        json!({"lease_id": "lease-1", "execution_id": "exec-1"}),
    )?;
    roundtrip(&PollCommandResponse::Running, json!({"outcome": "running"}))?;
    roundtrip(&PollCommandResponse::NotFound, json!({"outcome": "not_found"}))?;
    roundtrip(
        &PollCommandResponse::Completed(CommandOutput::default()),
        json!({"outcome": "completed", "exit_code": 0, "stdout": [], "stderr": []}),
    )?;
    roundtrip(
        &PollCommandResponse::Denied(LeaseError::NoProviderAvailable),
        json!({"outcome": "denied", "kind": "no_provider_available"}),
    )
}

#[test]
fn process_names_enforce_their_rules() -> TestResult {
    let name = ProcessName::new("web.1_a-b")?;
    assert_eq!(name.as_str(), "web.1_a-b");
    assert_eq!(name.to_string(), "web.1_a-b");
    assert_eq!("web".parse::<ProcessName>()?.as_str(), "web");
    assert_eq!(ProcessName::try_from("web".to_string())?.as_str(), "web");
    assert_eq!(String::from(name.clone()), "web.1_a-b");
    assert_eq!(ProcessName::new("a".repeat(ProcessName::MAX_LEN))?.as_str().len(), 64);
    assert_eq!(ProcessName::new(""), Err(ProcessNameError::Length { max: 64 }));
    assert_eq!(
        ProcessName::new("a".repeat(65)),
        Err(ProcessNameError::Length { max: 64 })
    );
    assert_eq!(ProcessName::new("a b"), Err(ProcessNameError::Characters));
    assert_eq!("a/b".parse::<ProcessName>(), Err(ProcessNameError::Characters));
    roundtrip(&name, json!("web.1_a-b"))?;
    rejected::<ProcessName>(json!("a b"))?;
    rejected::<ProcessSpec>(json!({"name": "a b"}))?;
    rejected::<ProcessInfo>(json!({"execution_id": "e", "name": "a b", "state": "running"}))
}

#[test]
fn process_name_error_messages() {
    assert_eq!(
        ProcessNameError::Length { max: 64 }.to_string(),
        "process name must be 1 to 64 characters"
    );
    assert_eq!(
        ProcessNameError::Characters.to_string(),
        "process name may only contain ASCII letters, digits, '.', '_' and '-'"
    );
    assert_eq!(
        ReadinessNeedleError::Length { max: 4096 }.to_string(),
        "readiness needle must be 1 to 4096 bytes"
    );
}

#[test]
fn readiness_needles_enforce_their_rules() -> TestResult {
    let needle = ReadinessNeedle::new("ready")?;
    assert_eq!(needle.as_str(), "ready");
    assert_eq!(needle.to_string(), "ready");
    assert_eq!("ready".parse::<ReadinessNeedle>()?, needle);
    assert_eq!(ReadinessNeedle::try_from("ready".to_string())?, needle);
    assert_eq!(String::from(needle.clone()), "ready");
    assert_eq!(
        ReadinessNeedle::new("a".repeat(ReadinessNeedle::MAX_LEN))?
            .as_str()
            .len(),
        4096
    );
    assert_eq!(
        ReadinessNeedle::new(""),
        Err(ReadinessNeedleError::Length { max: 4096 })
    );
    assert_eq!(
        "a".repeat(4097).parse::<ReadinessNeedle>(),
        Err(ReadinessNeedleError::Length { max: 4096 })
    );
    roundtrip(&needle, json!("ready"))?;
    rejected::<ReadinessNeedle>(json!(""))?;
    rejected::<ReadinessProbe>(json!({"kind": "output_contains", "needle": ""}))
}

#[test]
fn readiness_probes_are_tagged_by_kind() -> TestResult {
    roundtrip(
        &ReadinessProbe::TcpPort {
            port: NonZeroU16::new(80).ok_or("nonzero")?,
        },
        json!({"kind": "tcp_port", "port": 80}),
    )?;
    roundtrip(
        &ReadinessProbe::OutputContains {
            needle: ReadinessNeedle::new("up")?,
        },
        json!({"kind": "output_contains", "needle": "up"}),
    )?;
    rejected::<ReadinessProbe>(json!({"kind": "tcp_port", "port": 0}))
}

#[test]
fn process_specs_and_info() -> TestResult {
    assert_eq!(
        ProcessSpec::default(),
        ProcessSpec {
            name: None,
            readiness: None
        }
    );
    roundtrip(&ProcessSpec::default(), json!({}))?;
    roundtrip(&ProcessState::Running, json!({"state": "running"}))?;
    roundtrip(
        &ProcessState::Exited { exit_code: 2 },
        json!({"state": "exited", "exit_code": 2}),
    )?;
    roundtrip(
        &ProcessInfo {
            execution_id: exec(),
            name: None,
            readiness: None,
            state: ProcessState::Running,
        },
        json!({"execution_id": "exec-1", "state": "running"}),
    )?;
    roundtrip(
        &ProcessInfo {
            execution_id: exec(),
            name: Some(ProcessName::new("web")?),
            readiness: Some(ReadinessProbe::OutputContains {
                needle: ReadinessNeedle::new("up")?,
            }),
            state: ProcessState::Exited { exit_code: 1 },
        },
        json!({
            "execution_id": "exec-1",
            "name": "web",
            "readiness": {"kind": "output_contains", "needle": "up"},
            "state": "exited",
            "exit_code": 1,
        }),
    )
}

#[test]
fn listing_processes() -> TestResult {
    roundtrip(
        &ListProcessesRequest { lease_id: lease() },
        json!({"lease_id": "lease-1"}),
    )?;
    roundtrip(
        &ListProcessesResponse::Processes {
            processes: vec![ProcessInfo {
                execution_id: exec(),
                name: None,
                readiness: None,
                state: ProcessState::Running,
            }],
        },
        json!({"outcome": "processes", "processes": [{"execution_id": "exec-1", "state": "running"}]}),
    )?;
    roundtrip(&ListProcessesResponse::Unsupported, json!({"outcome": "unsupported"}))?;
    roundtrip(
        &ListProcessesResponse::Denied(LeaseError::NoProviderAvailable),
        json!({"outcome": "denied", "kind": "no_provider_available"}),
    )
}

#[test]
fn waiting_for_a_process() -> TestResult {
    let parsed: WaitReadyRequest = serde_json::from_value(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?;
    assert_eq!(parsed.timeout_seconds, WaitReadyRequest::DEFAULT_TIMEOUT_SECONDS);
    assert_eq!(WaitReadyRequest::DEFAULT_TIMEOUT_SECONDS, 30);
    assert_eq!(WaitReadyRequest::MAX_TIMEOUT_SECONDS, 300);
    roundtrip(
        &parsed,
        json!({"lease_id": "lease-1", "execution_id": "exec-1", "timeout_seconds": 30}),
    )?;
    for (response, wire) in [
        (WaitReadyResponse::Ready, json!({"outcome": "ready"})),
        (WaitReadyResponse::NotReady, json!({"outcome": "not_ready"})),
        (
            WaitReadyResponse::Failed { exit_code: 1 },
            json!({"outcome": "failed", "exit_code": 1}),
        ),
        (WaitReadyResponse::NoProbe, json!({"outcome": "no_probe"})),
        (WaitReadyResponse::NotFound, json!({"outcome": "not_found"})),
        (WaitReadyResponse::Unsupported, json!({"outcome": "unsupported"})),
        (
            WaitReadyResponse::Denied(LeaseError::NoProviderAvailable),
            json!({"outcome": "denied", "kind": "no_provider_available"}),
        ),
    ] {
        roundtrip(&response, wire)?;
    }
    Ok(())
}

#[test]
fn ready_ports_enforce_their_rules() -> TestResult {
    let ports = ReadyPorts::new(vec![80, 443])?;
    assert_eq!(ports.as_slice(), &[80, 443]);
    assert_eq!(ReadyPorts::default().as_slice(), &[] as &[u16]);
    assert_eq!(ReadyPorts::try_from(vec![1])?.as_slice(), &[1]);
    assert_eq!(Vec::from(ports.clone()), vec![80, 443]);
    assert_eq!(
        ReadyPorts::new((1..=16).collect())?.as_slice().len(),
        ReadyPorts::MAX_LEN
    );
    assert_eq!(
        ReadyPorts::new((1..=17).collect()),
        Err(ReadyPortsError::TooMany { max: 16 })
    );
    assert_eq!(ReadyPorts::new(vec![0]), Err(ReadyPortsError::ZeroPort));
    assert_eq!(ReadyPorts::new(vec![5, 5]), Err(ReadyPortsError::Duplicate));
    roundtrip(&ports, json!([80, 443]))?;
    rejected::<ReadyPorts>(json!([0]))?;
    rejected::<WaitSandboxReadyRequest>(json!({"lease_id": "l", "ports": [7, 7]}))
}

#[test]
fn ready_ports_error_messages() {
    for (error, message) in [
        (ReadyPortsError::TooMany { max: 16 }, "at most 16 ports may be checked"),
        (ReadyPortsError::ZeroPort, "port 0 cannot be checked"),
        (ReadyPortsError::Duplicate, "duplicate ports"),
    ] {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn waiting_for_a_sandbox() -> TestResult {
    let parsed: WaitSandboxReadyRequest = serde_json::from_value(json!({"lease_id": "lease-1"}))?;
    assert_eq!(parsed.ports, ReadyPorts::default());
    assert_eq!(parsed.timeout_seconds, WaitSandboxReadyRequest::DEFAULT_TIMEOUT_SECONDS);
    assert_eq!(WaitSandboxReadyRequest::DEFAULT_TIMEOUT_SECONDS, 30);
    assert_eq!(WaitSandboxReadyRequest::MAX_TIMEOUT_SECONDS, 300);
    roundtrip(
        &parsed,
        json!({"lease_id": "lease-1", "ports": [], "timeout_seconds": 30}),
    )?;
    roundtrip(
        &WaitSandboxReadyRequest {
            lease_id: lease(),
            ports: ReadyPorts::new(vec![8080])?,
            timeout_seconds: 9,
        },
        json!({"lease_id": "lease-1", "ports": [8080], "timeout_seconds": 9}),
    )?;
    roundtrip(&WaitSandboxReadyResponse::Ready, json!({"outcome": "ready"}))?;
    roundtrip(
        &WaitSandboxReadyResponse::NotReady {
            pending_ports: vec![80],
        },
        json!({"outcome": "not_ready", "pending_ports": [80]}),
    )?;
    let older: WaitSandboxReadyResponse = serde_json::from_value(json!({"outcome": "not_ready"}))?;
    assert_eq!(
        older,
        WaitSandboxReadyResponse::NotReady {
            pending_ports: Vec::new()
        }
    );
    roundtrip(
        &WaitSandboxReadyResponse::Unsupported,
        json!({"outcome": "unsupported"}),
    )?;
    roundtrip(
        &WaitSandboxReadyResponse::Denied(LeaseError::NoProviderAvailable),
        json!({"outcome": "denied", "kind": "no_provider_available"}),
    )
}

#[test]
fn command_env_rejects_the_reserved_key() -> TestResult {
    assert_eq!(CommandEnv::RESERVED_KEY, "SANDBOX_EXECUTION_ID");
    let vars = BTreeMap::from([("A".to_string(), "b".to_string())]);
    let env = CommandEnv::new(vars.clone())?;
    assert_eq!(env.as_map(), &vars);
    assert_eq!(env.iter().count(), 1);
    assert!(!env.is_empty());
    assert!(CommandEnv::default().is_empty());
    assert_eq!(BTreeMap::from(env.clone()), vars);
    assert_eq!(CommandEnv::try_from(vars)?, env);
    roundtrip(&env, json!({"A": "b"}))?;
    let reserved = BTreeMap::from([("SANDBOX_EXECUTION_ID".to_string(), "x".to_string())]);
    assert_eq!(CommandEnv::new(reserved), Err(CommandEnvError::ReservedKey));
    assert_eq!(
        CommandEnvError::ReservedKey.to_string(),
        "environment key SANDBOX_EXECUTION_ID is reserved"
    );
    rejected::<CommandEnv>(json!({"SANDBOX_EXECUTION_ID": "x"}))?;
    let mut body = minimal_json();
    body["env"] = json!({"SANDBOX_EXECUTION_ID": "x"});
    rejected::<RunCommandRequest>(body)
}

#[test]
fn redact_values_enforce_their_limits() -> TestResult {
    let values = RedactValues::new(vec![RedactValue::new("short"), RedactValue::new("x".repeat(8192))])?;
    assert_eq!(values.as_slice().len(), 2);
    assert_eq!(values.iter().count(), 2);
    assert!(!values.is_empty());
    assert!(RedactValues::default().is_empty());
    assert_eq!(Vec::from(values.clone()).len(), 2);
    assert_eq!(RedactValues::try_from(Vec::from(values.clone()))?, values);
    let many = |n: usize| (0..n).map(|i| RedactValue::new(i.to_string())).collect::<Vec<_>>();
    assert_eq!(RedactValues::new(many(64))?.as_slice().len(), RedactValues::MAX_VALUES);
    assert_eq!(RedactValues::new(many(65)), Err(RedactValuesError::TooMany { max: 64 }));
    assert_eq!(
        RedactValues::new(vec![RedactValue::new("x".repeat(RedactValues::MAX_VALUE_LEN + 1))]),
        Err(RedactValuesError::TooLong { max: 8192 })
    );
    roundtrip(&RedactValues::new(vec![RedactValue::new("abc")])?, json!(["abc"]))?;
    rejected::<RedactValues>(Value::Array(vec![json!("x"); 65]))?;
    let mut body = minimal_json();
    body["redact_values"] = json!(["x".repeat(8193)]);
    rejected::<RunCommandRequest>(body)
}

#[test]
fn redact_values_error_messages() {
    assert_eq!(
        RedactValuesError::TooMany { max: 64 }.to_string(),
        "at most 64 values can be registered for redaction"
    );
    assert_eq!(
        RedactValuesError::TooLong { max: 8192 }.to_string(),
        "a value registered for redaction exceeds 8192 bytes"
    );
}
