use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CommandExitReport, ExecutionId, KillCommandRequest,
    KillCommandResponse, LaunchKey, LaunchKeyError, LeaseError, LeaseId, LogChunk, LogsContinuation, OutputRetention,
    OutputStream, OutputTruncation, ReadLogsRequest, ReadLogsResponse, RestartCommandRequest, RestartCommandResponse,
    StdinChunk, StdinChunkError, StdinEof, StdinMode, StdinSeq, StopCommandRequest, StopCommandResponse, StopSignal,
    UnixSignal, WriteStdinRequest, WriteStdinRequestError, WriteStdinResponse,
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

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn exec() -> ExecutionId {
    ExecutionId::new("exec-1")
}

fn denied() -> (LeaseError, Value) {
    (LeaseError::NoProviderAvailable, json!("no_provider_available"))
}

#[test]
fn stdin_mode_and_output_retention_use_snake_case_and_default() -> TestResult {
    assert_eq!(StdinMode::default(), StdinMode::Closed);
    assert_eq!(OutputRetention::default(), OutputRetention::Buffer);
    roundtrip(&StdinMode::Closed, json!("closed"))?;
    roundtrip(&StdinMode::Interactive, json!("interactive"))?;
    roundtrip(&OutputRetention::Buffer, json!("buffer"))?;
    roundtrip(&OutputRetention::StreamOnly, json!("stream_only"))?;
    assert_eq!(OutputRetention::STREAM_ONLY_STDERR_TAIL_BYTES, 65536);
    Ok(())
}

#[test]
fn execution_id_accepts_any_string() -> TestResult {
    let id = ExecutionId::new("exec-1");
    assert_eq!(id.as_str(), "exec-1");
    assert_eq!(id.to_string(), "exec-1");
    assert_eq!(ExecutionId::from("exec-1".to_string()), id);
    assert_eq!(String::from(id.clone()), "exec-1");
    roundtrip(&id, json!("exec-1"))
}

#[test]
fn stdin_seq_serializes_as_a_number() -> TestResult {
    let seq = StdinSeq::new(7);
    assert_eq!(seq.get(), 7);
    assert_eq!(StdinSeq::default().get(), 0);
    roundtrip(&seq, json!(7))
}

#[test]
fn stdin_chunk_enforces_its_limit() -> TestResult {
    let max = StdinChunk::new(vec![1; StdinChunk::MAX_LEN])?;
    assert_eq!(max.as_slice().len(), StdinChunk::MAX_LEN);
    assert_eq!(StdinChunk::default().as_slice(), &[] as &[u8]);
    assert_eq!(
        StdinChunk::new(vec![0; StdinChunk::MAX_LEN + 1]),
        Err(StdinChunkError::TooLarge { max: 65536 })
    );
    assert_eq!(Vec::from(StdinChunk::new(vec![1, 2])?), vec![1, 2]);
    roundtrip(&StdinChunk::new(vec![1, 2])?, json!([1, 2]))?;
    let too_large = serde_json::from_value::<StdinChunk>(Value::Array(vec![json!(0); StdinChunk::MAX_LEN + 1]))
        .err()
        .ok_or("an oversized chunk must be rejected")?;
    assert_eq!(too_large.classify(), Data);
    Ok(())
}

#[test]
fn stdin_chunk_error_messages() {
    assert_eq!(
        StdinChunkError::TooLarge { max: 65536 }.to_string(),
        "stdin chunk exceeds 65536 bytes"
    );
    assert_eq!(
        WriteStdinRequestError::Empty.to_string(),
        "stdin write must carry data or eof"
    );
}

#[test]
fn write_stdin_request_requires_data_or_eof() -> TestResult {
    let data = StdinChunk::new(vec![104])?;
    let request = WriteStdinRequest::new(lease(), exec(), StdinSeq::new(0), data.clone(), StdinEof::Keep)?;
    assert_eq!(request.lease_id(), &lease());
    assert_eq!(request.execution_id(), &exec());
    assert_eq!(request.seq(), StdinSeq::new(0));
    assert_eq!(request.data(), &data);
    assert_eq!(request.eof(), StdinEof::Keep);
    roundtrip(
        &request,
        json!({"lease_id": "lease-1", "execution_id": "exec-1", "seq": 0, "data": [104], "eof": false}),
    )?;

    let close = WriteStdinRequest::new(
        lease(),
        exec(),
        StdinSeq::new(1),
        StdinChunk::default(),
        StdinEof::Close,
    )?;
    roundtrip(
        &close,
        json!({"lease_id": "lease-1", "execution_id": "exec-1", "seq": 1, "data": [], "eof": true}),
    )?;

    assert_eq!(
        WriteStdinRequest::new(lease(), exec(), StdinSeq::new(2), StdinChunk::default(), StdinEof::Keep),
        Err(WriteStdinRequestError::Empty)
    );
    Ok(())
}

#[test]
fn write_stdin_request_deserialization_enforces_the_rules() -> TestResult {
    let defaults: WriteStdinRequest = serde_json::from_value(json!({
        "lease_id": "lease-1", "execution_id": "exec-1", "seq": 0, "eof": true
    }))?;
    assert_eq!(defaults.data().as_slice(), &[] as &[u8]);
    assert_eq!(defaults.eof(), StdinEof::Close);

    let empty = serde_json::from_value::<WriteStdinRequest>(json!({
        "lease_id": "lease-1", "execution_id": "exec-1", "seq": 0
    }))
    .err()
    .ok_or("an empty write must be rejected")?;
    assert_eq!(empty.classify(), Data);

    let oversized = serde_json::from_value::<WriteStdinRequest>(json!({
        "lease_id": "lease-1", "execution_id": "exec-1", "seq": 0,
        "data": vec![0; StdinChunk::MAX_LEN + 1]
    }))
    .err()
    .ok_or("an oversized write must be rejected")?;
    assert_eq!(oversized.classify(), Data);
    Ok(())
}

#[test]
fn stdin_eof_is_a_boolean_on_the_wire() -> TestResult {
    assert_eq!(StdinEof::default(), StdinEof::Keep);
    assert!(!StdinEof::Keep.closes());
    assert!(StdinEof::Close.closes());
    roundtrip(&StdinEof::Keep, json!(false))?;
    roundtrip(&StdinEof::Close, json!(true))
}

#[test]
fn write_stdin_response_variants() -> TestResult {
    let next = StdinSeq::new(3);
    for (response, outcome) in [
        (WriteStdinResponse::Written { next_seq: next }, "written"),
        (WriteStdinResponse::Duplicate { next_seq: next }, "duplicate"),
        (WriteStdinResponse::Buffered { next_seq: next }, "buffered"),
        (WriteStdinResponse::Backpressure { next_seq: next }, "backpressure"),
        (WriteStdinResponse::OutOfWindow { next_seq: next }, "out_of_window"),
    ] {
        roundtrip(&response, json!({"outcome": outcome, "next_seq": 3}))?;
    }
    for (response, outcome) in [
        (WriteStdinResponse::StdinClosed, "stdin_closed"),
        (WriteStdinResponse::StdinNotInteractive, "stdin_not_interactive"),
        (WriteStdinResponse::AlreadyCompleted, "already_completed"),
        (WriteStdinResponse::NotFound, "not_found"),
        (WriteStdinResponse::Unsupported, "unsupported"),
    ] {
        roundtrip(&response, json!({"outcome": outcome}))?;
    }
    let (error, kind) = denied();
    roundtrip(
        &WriteStdinResponse::Denied(error),
        json!({"outcome": "denied", "kind": kind}),
    )
}

#[test]
fn kill_command_request_defaults_to_term() -> TestResult {
    let parsed: KillCommandRequest = serde_json::from_value(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?;
    assert_eq!(parsed.signal, UnixSignal::Term);
    roundtrip(
        &KillCommandRequest {
            lease_id: lease(),
            execution_id: exec(),
            signal: UnixSignal::Kill,
        },
        json!({"lease_id": "lease-1", "execution_id": "exec-1", "signal": "kill"}),
    )
}

#[test]
fn unix_signals_use_snake_case_and_kill_names() -> TestResult {
    for (signal, wire, name) in [
        (UnixSignal::Term, "term", "TERM"),
        (UnixSignal::Kill, "kill", "KILL"),
        (UnixSignal::Int, "int", "INT"),
        (UnixSignal::Hup, "hup", "HUP"),
        (UnixSignal::Quit, "quit", "QUIT"),
        (UnixSignal::Usr1, "usr1", "USR1"),
        (UnixSignal::Usr2, "usr2", "USR2"),
    ] {
        roundtrip(&signal, json!(wire))?;
        assert_eq!(signal.as_str(), name);
    }
    let error = serde_json::from_value::<UnixSignal>(json!("rm -rf"))
        .err()
        .ok_or("an arbitrary signal name must be rejected")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn kill_command_response_variants() -> TestResult {
    roundtrip(&KillCommandResponse::Signaled, json!({"outcome": "signaled"}))?;
    roundtrip(
        &KillCommandResponse::AlreadyCompleted,
        json!({"outcome": "already_completed"}),
    )?;
    roundtrip(&KillCommandResponse::NotFound, json!({"outcome": "not_found"}))?;
    let (error, kind) = denied();
    roundtrip(
        &KillCommandResponse::Denied(error),
        json!({"outcome": "denied", "kind": kind}),
    )
}

#[test]
fn launch_key_enforces_its_rules() -> TestResult {
    let key = LaunchKey::new("job_1-a")?;
    assert_eq!(key.as_str(), "job_1-a");
    assert_eq!(key.to_string(), "job_1-a");
    assert_eq!("job_1-a".parse::<LaunchKey>()?, key);
    assert_eq!(LaunchKey::try_from("job_1-a".to_string())?, key);
    assert_eq!(String::from(key.clone()), "job_1-a");
    assert_eq!(LaunchKey::new("a".repeat(LaunchKey::MAX_LEN))?.as_str().len(), 128);
    assert_eq!(LaunchKey::new(""), Err(LaunchKeyError::Empty));
    assert_eq!(
        LaunchKey::new("a".repeat(LaunchKey::MAX_LEN + 1)),
        Err(LaunchKeyError::TooLong { max: 128 })
    );
    assert_eq!(LaunchKey::new("a.b"), Err(LaunchKeyError::InvalidCharacters));
    assert_eq!("a b".parse::<LaunchKey>(), Err(LaunchKeyError::InvalidCharacters));
    roundtrip(&key, json!("job_1-a"))?;
    let error = serde_json::from_value::<LaunchKey>(json!("a/b"))
        .err()
        .ok_or("an invalid key must be rejected")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn launch_key_error_messages() {
    for (error, message) in [
        (LaunchKeyError::Empty, "launch key must not be empty"),
        (
            LaunchKeyError::TooLong { max: 128 },
            "launch key must be at most 128 characters",
        ),
        (
            LaunchKeyError::InvalidCharacters,
            "launch key may only contain ASCII letters, digits, '_' and '-'",
        ),
    ] {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn cancel_command_launch_request_validates_its_key_on_deserialization() -> TestResult {
    roundtrip(
        &CancelCommandLaunchRequest {
            lease_id: lease(),
            launch_key: LaunchKey::new("k1")?,
        },
        json!({"lease_id": "lease-1", "launch_key": "k1"}),
    )?;
    let error = serde_json::from_value::<CancelCommandLaunchRequest>(json!({"lease_id": "l", "launch_key": ""}))
        .err()
        .ok_or("an empty key must be rejected")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn cancel_command_launch_response_variants() -> TestResult {
    roundtrip(
        &CancelCommandLaunchResponse::Canceled { execution_id: None },
        json!({"outcome": "canceled"}),
    )?;
    roundtrip(
        &CancelCommandLaunchResponse::Canceled {
            execution_id: Some(exec()),
        },
        json!({"outcome": "canceled", "execution_id": "exec-1"}),
    )?;
    roundtrip(
        &CancelCommandLaunchResponse::Preserved { execution_id: exec() },
        json!({"outcome": "preserved", "execution_id": "exec-1"}),
    )?;
    roundtrip(
        &CancelCommandLaunchResponse::Pending {
            message: "later".to_string(),
        },
        json!({"outcome": "pending", "message": "later"}),
    )?;
    roundtrip(
        &CancelCommandLaunchResponse::Unsupported,
        json!({"outcome": "unsupported"}),
    )?;
    let (error, kind) = denied();
    roundtrip(
        &CancelCommandLaunchResponse::Denied(error),
        json!({"outcome": "denied", "kind": kind}),
    )
}

#[test]
fn stop_command_request_defaults_its_grace_period() -> TestResult {
    let parsed: StopCommandRequest = serde_json::from_value(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?;
    assert_eq!(parsed.grace_seconds, StopCommandRequest::DEFAULT_GRACE_SECONDS);
    assert_eq!(StopCommandRequest::DEFAULT_GRACE_SECONDS, 10);
    assert_eq!(StopCommandRequest::MAX_GRACE_SECONDS, 300);
    roundtrip(
        &StopCommandRequest {
            lease_id: lease(),
            execution_id: exec(),
            grace_seconds: 5,
        },
        json!({"lease_id": "lease-1", "execution_id": "exec-1", "grace_seconds": 5}),
    )
}

#[test]
fn stop_command_response_variants() -> TestResult {
    roundtrip(&StopSignal::Term, json!("term"))?;
    roundtrip(&StopSignal::Kill, json!("kill"))?;
    roundtrip(
        &StopCommandResponse::Stopped {
            signal: StopSignal::Kill,
        },
        json!({"outcome": "stopped", "signal": "kill"}),
    )?;
    roundtrip(
        &StopCommandResponse::AlreadyCompleted,
        json!({"outcome": "already_completed"}),
    )?;
    roundtrip(&StopCommandResponse::NotFound, json!({"outcome": "not_found"}))?;
    roundtrip(&StopCommandResponse::Unsupported, json!({"outcome": "unsupported"}))?;
    let (error, kind) = denied();
    roundtrip(
        &StopCommandResponse::Denied(error),
        json!({"outcome": "denied", "kind": kind}),
    )
}

#[test]
fn restart_command_request_defaults_its_grace_period() -> TestResult {
    let parsed: RestartCommandRequest =
        serde_json::from_value(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?;
    assert_eq!(parsed.grace_seconds, 10);
    roundtrip(
        &RestartCommandRequest {
            lease_id: lease(),
            execution_id: exec(),
            grace_seconds: 2,
        },
        json!({"lease_id": "lease-1", "execution_id": "exec-1", "grace_seconds": 2}),
    )
}

#[test]
fn restart_command_response_variants() -> TestResult {
    roundtrip(
        &RestartCommandResponse::Restarted { execution_id: exec() },
        json!({"outcome": "restarted", "execution_id": "exec-1"}),
    )?;
    roundtrip(&RestartCommandResponse::NotFound, json!({"outcome": "not_found"}))?;
    roundtrip(&RestartCommandResponse::Unsupported, json!({"outcome": "unsupported"}))?;
    let (error, kind) = denied();
    roundtrip(
        &RestartCommandResponse::Denied(error),
        json!({"outcome": "denied", "kind": kind}),
    )
}

#[test]
fn read_logs_request_defaults_and_omits_unset_fields() -> TestResult {
    let parsed: ReadLogsRequest = serde_json::from_value(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?;
    assert_eq!(parsed.after_seq, 0);
    assert_eq!(parsed.max_bytes, None);
    assert_eq!(parsed.wait_ms, None);
    assert_eq!(ReadLogsRequest::DEFAULT_BYTES, 65536);
    assert_eq!(ReadLogsRequest::MAX_BYTES, 524288);
    assert_eq!(ReadLogsRequest::MAX_WAIT_MS, 20000);
    roundtrip(
        &parsed,
        json!({"lease_id": "lease-1", "execution_id": "exec-1", "after_seq": 0}),
    )?;
    roundtrip(
        &ReadLogsRequest {
            lease_id: lease(),
            execution_id: exec(),
            after_seq: 4,
            max_bytes: Some(10),
            wait_ms: Some(500),
        },
        json!({
            "lease_id": "lease-1", "execution_id": "exec-1", "after_seq": 4, "max_bytes": 10, "wait_ms": 500
        }),
    )
}

#[test]
fn read_logs_response_variants() -> TestResult {
    let chunk = LogChunk {
        log_seq: 1,
        stream: OutputStream::Stderr,
        seq: 0,
        data: vec![101],
    };
    roundtrip(
        &chunk,
        json!({"log_seq": 1, "stream": "stderr", "seq": 0, "data": [101]}),
    )?;
    roundtrip(&OutputStream::Stdout, json!("stdout"))?;

    roundtrip(
        &ReadLogsResponse::Logs {
            chunks: vec![chunk.clone()],
            next_seq: 1,
            has_more: LogsContinuation::HasMore,
            truncated: OutputTruncation::Truncated,
            exited: CommandExitReport::Reported,
        },
        json!({
            "outcome": "logs",
            "chunks": [{"log_seq": 1, "stream": "stderr", "seq": 0, "data": [101]}],
            "next_seq": 1,
            "has_more": true,
            "truncated": true,
            "exited": true,
        }),
    )?;
    let older: ReadLogsResponse = serde_json::from_value(json!({"outcome": "logs", "chunks": [], "next_seq": 0}))?;
    assert_eq!(
        older,
        ReadLogsResponse::Logs {
            chunks: Vec::new(),
            next_seq: 0,
            has_more: LogsContinuation::Complete,
            truncated: OutputTruncation::Complete,
            exited: CommandExitReport::NotReported,
        }
    );
    roundtrip(&ReadLogsResponse::Unsupported, json!({"outcome": "unsupported"}))?;
    let (error, kind) = denied();
    roundtrip(
        &ReadLogsResponse::Denied(error),
        json!({"outcome": "denied", "kind": kind}),
    )
}

#[test]
fn log_flags_are_booleans_on_the_wire() -> TestResult {
    assert!(LogsContinuation::HasMore.has_more());
    assert!(!LogsContinuation::Complete.has_more());
    assert!(CommandExitReport::Reported.is_reported());
    assert!(!CommandExitReport::NotReported.is_reported());
    roundtrip(&LogsContinuation::Complete, json!(false))?;
    roundtrip(&CommandExitReport::NotReported, json!(false))
}
