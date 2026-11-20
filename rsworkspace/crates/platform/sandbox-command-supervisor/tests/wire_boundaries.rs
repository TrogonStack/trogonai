use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, BufReader, Cursor, Read, Write};

use sandbox_command_supervisor::protocol::{
    Cancellation, Certainty, Command, CommandWire, Delivery, Event, ExecutionId, MAX_ARGUMENT_BYTES,
    MAX_COMMAND_TIMEOUT_MS, MAX_ENV_BYTES, MAX_RELEASE_TIMEOUT_MS, PlatformSupport, ProtocolError, ReleaseState,
    TimeoutState, Truncation, VERSION, decode_command, decode_event, encode_event, read_event, write_command,
};

type TestResult = Result<(), Box<dyn Error>>;

fn init() -> Result<CommandWire, Box<dyn Error>> {
    Ok(CommandWire::Init {
        version: VERSION,
        execution_id: ExecutionId::new("boundary")?,
        argv: vec!["/bin/true".into()],
        env: BTreeMap::new(),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    })
}

fn accepts(command: CommandWire) -> Result<bool, Box<dyn Error>> {
    let bytes = serde_json::to_vec(&command)?;
    Ok(decode_command(&bytes).is_ok())
}

#[test]
fn argument_count_bytes_and_nul_are_checked_at_the_json_boundary() -> TestResult {
    for (argv, expected) in [
        (Vec::new(), false),
        (vec![String::new()], false),
        (vec!["x\0y".into()], false),
        (vec!["x".repeat(MAX_ARGUMENT_BYTES)], true),
        (vec!["x".repeat(MAX_ARGUMENT_BYTES + 1)], false),
        (vec!["x".into(); 1024], true),
        (vec!["x".into(); 1025], false),
        (vec!["x".into(), String::new()], true),
    ] {
        let mut command = init()?;
        if let CommandWire::Init { argv: value, .. } = &mut command {
            *value = argv;
        }
        assert_eq!(accepts(command)?, expected);
    }
    Ok(())
}

#[test]
fn environment_keys_values_count_and_total_bytes_keep_the_wire_bounds() -> TestResult {
    for (env, expected) in [
        (BTreeMap::from([(String::new(), "x".into())]), false),
        (BTreeMap::from([("a=b".into(), "x".into())]), false),
        (BTreeMap::from([("a\0b".into(), "x".into())]), false),
        (BTreeMap::from([("a".into(), "x\0y".into())]), false),
        (BTreeMap::from([("x".repeat(8192), String::new())]), true),
        (BTreeMap::from([("x".repeat(8193), String::new())]), false),
        (BTreeMap::from([("a".into(), "x".repeat(MAX_ENV_BYTES - 3))]), true),
        (BTreeMap::from([("a".into(), "x".repeat(MAX_ENV_BYTES - 2))]), false),
        (
            (0..1024).map(|index| (index.to_string(), String::new())).collect(),
            true,
        ),
        (
            (0..1025).map(|index| (index.to_string(), String::new())).collect(),
            false,
        ),
    ] {
        let mut command = init()?;
        if let CommandWire::Init { env: value, .. } = &mut command {
            *value = env;
        }
        assert_eq!(accepts(command)?, expected);
    }
    Ok(())
}

#[test]
fn directory_and_deadline_bounds_preserve_optional_and_exact_maxima() -> TestResult {
    for (cwd, expected) in [
        (None, true),
        (Some(String::new()), false),
        (Some("a\0b".into()), false),
        (Some("x".repeat(4096)), true),
        (Some("x".repeat(4097)), false),
    ] {
        let mut command = init()?;
        if let CommandWire::Init { cwd: value, .. } = &mut command {
            *value = cwd;
        }
        assert_eq!(accepts(command)?, expected);
    }
    for (timeout, expected) in [
        (0, false),
        (1, true),
        (MAX_COMMAND_TIMEOUT_MS, true),
        (MAX_COMMAND_TIMEOUT_MS + 1, false),
    ] {
        let mut command = init()?;
        if let CommandWire::Init { timeout_ms, .. } = &mut command {
            *timeout_ms = Some(timeout);
        }
        assert_eq!(accepts(command)?, expected);
    }
    for (timeout, expected) in [
        (0, false),
        (1, true),
        (MAX_RELEASE_TIMEOUT_MS, true),
        (MAX_RELEASE_TIMEOUT_MS + 1, false),
    ] {
        let mut command = init()?;
        if let CommandWire::Init { release_timeout_ms, .. } = &mut command {
            *release_timeout_ms = Some(timeout);
        }
        assert_eq!(accepts(command)?, expected);
    }
    Ok(())
}

#[test]
fn signal_bounds_and_all_command_identities_survive_conversion() -> TestResult {
    for (request_id, signal, expected) in [
        (0, 15, false),
        (1, 0, false),
        (1, 65, false),
        (1, -1, false),
        (1, 1, true),
        (u64::MAX, 64, true),
    ] {
        assert_eq!(
            accepts(CommandWire::Signal {
                execution_id: ExecutionId::new("boundary")?,
                request_id,
                signal
            })?,
            expected
        );
    }
    for kind in ["release", "status", "acknowledge"] {
        let bytes = format!("{{\"kind\":\"{kind}\",\"execution_id\":\"boundary\"}}");
        let command = decode_command(bytes.as_bytes())?;
        assert_eq!(command.execution_id().as_str(), "boundary");
    }
    let encoded = serde_json::to_vec(&init()?)?;
    let command = decode_command(&encoded)?;
    assert_eq!(command.execution_id().as_str(), "boundary");
    let Command::Init { version, .. } = command else {
        return Err(io::Error::other("Init must decode to the typed initialization command").into());
    };
    assert_eq!(version.get(), VERSION);
    assert!(decode_command(br#"{"kind":"release","execution_id":""}"#).is_err());
    assert!(decode_command(br#"{"kind":"unknown","execution_id":"boundary"}"#).is_err());
    Ok(())
}

#[test]
fn event_flags_retain_boolean_json_and_hello_environment_is_bounded() -> TestResult {
    for (index, bytes) in [
        br#"{"kind":"probe","version":1,"supported":true}"#.as_slice(),
        br#"{"kind":"probe","version":1,"supported":false}"#,
        br#"{"kind":"signal_result","execution_id":"boundary","request_id":1,"signal":15,"delivered":true}"#,
        br#"{"kind":"error","execution_id":"boundary","code":"invalid_command","uncertain":false}"#,
        br#"{"kind":"family_absent","execution_id":"boundary","released":true,"root_exit_code":0,"root_signal":null,"timed_out":false,"canceled":false,"reaped_count":1,"stdout_truncated":false,"stderr_truncated":true}"#,
        br#"{"kind":"family_absent","execution_id":"boundary","released":false,"root_exit_code":null,"root_signal":9,"timed_out":true,"canceled":true,"reaped_count":1,"stdout_truncated":true,"stderr_truncated":false}"#,
    ]
    .into_iter()
    .enumerate()
    {
        let event = decode_event(bytes)?;
        assert!(matches!(
            (&event, index),
            (Event::Probe { supported: PlatformSupport::Supported, .. }, 0)
                | (Event::Probe { supported: PlatformSupport::Unsupported, .. }, 1)
                | (Event::SignalResult { delivered: Delivery::Delivered, .. }, 2)
                | (Event::Error { uncertain: Certainty::Certain, .. }, 3)
                | (Event::FamilyAbsent {
                    released: ReleaseState::Released,
                    timed_out: TimeoutState::WithinDeadline,
                    canceled: Cancellation::Active,
                    stdout_truncated: Truncation::Complete,
                    stderr_truncated: Truncation::Truncated,
                    ..
                }, 4)
                | (Event::FamilyAbsent {
                    released: ReleaseState::Held,
                    timed_out: TimeoutState::TimedOut,
                    canceled: Cancellation::Canceled,
                    stdout_truncated: Truncation::Truncated,
                    stderr_truncated: Truncation::Complete,
                    ..
                }, 5)
        ));
        let encoded = encode_event(&event)?;
        assert_eq!(serde_json::from_slice::<serde_json::Value>(bytes)?, serde_json::from_slice::<serde_json::Value>(&encoded)?);
    }
    assert!(
        decode_event(br#"{"kind":"hello","version":1,"execution_id":"boundary","environment":{"BAD=KEY":"x"}}"#)
            .is_err()
    );
    assert!(decode_event(br#"{"kind":"probe","version":1,"supported":"true"}"#).is_err());
    assert!(decode_event(br#"{"kind":"probe","version":1,"supported":true,"unexpected":0}"#).is_err());
    assert!(matches!(
        decode_event(br#"{"kind":"hello","version":1,"execution_id":"boundary","environment":{}}"#),
        Ok(Event::Hello { .. })
    ));
    Ok(())
}

struct FailingIo;

impl Write for FailingIo {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

impl Read for FailingIo {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
}

#[test]
fn transport_errors_preserve_their_typed_sources_and_empty_eof_is_distinct() -> TestResult {
    let error = match write_command(&mut FailingIo, &init()?) {
        Err(error) => error,
        Ok(()) => return Err(io::Error::other("expected write failure").into()),
    };
    assert!(error.source().is_some_and(|source| source.is::<io::Error>()));
    let error = match read_event(&mut BufReader::new(FailingIo)) {
        Err(error) => error,
        Ok(_) => return Err(io::Error::other("expected read failure").into()),
    };
    assert!(error.source().is_some_and(|source| source.is::<io::Error>()));
    let error = match decode_command(b"{") {
        Err(error) => error,
        Ok(_) => return Err(io::Error::other("expected JSON failure").into()),
    };
    assert!(matches!(error, ProtocolError::InvalidJson(_)));
    assert!(error.source().is_some_and(|source| source.is::<serde_json::Error>()));
    assert!(read_event(&mut Cursor::new(Vec::<u8>::new()))?.is_none());
    Ok(())
}

#[test]
fn escaped_json_bytes_cannot_bypass_frame_bounds() -> TestResult {
    assert!(matches!(
        decode_event(&vec![b' '; sandbox_command_supervisor::protocol::MAX_FRAME_BYTES + 1]),
        Err(ProtocolError::OversizedFrame)
    ));
    let event = Event::Output {
        execution_id: ExecutionId::new("boundary")?,
        stream: sandbox_command_supervisor::protocol::Stream::Stdout,
        data: vec![255; sandbox_command_supervisor::protocol::MAX_FRAME_BYTES / 2],
    };
    assert!(matches!(encode_event(&event), Err(ProtocolError::OversizedFrame)));
    let mut command = init()?;
    if let CommandWire::Init { argv, .. } = &mut command {
        *argv = vec!["\u{1}".repeat(MAX_ARGUMENT_BYTES)];
    }
    assert!(matches!(
        write_command(&mut Vec::new(), &command),
        Err(ProtocolError::OversizedFrame)
    ));
    Ok(())
}

struct FailingFlush;

impl Write for FailingFlush {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

#[test]
fn a_successful_write_still_requires_a_successful_flush() -> TestResult {
    let error = match write_command(&mut FailingFlush, &init()?) {
        Err(error) => error,
        Ok(()) => return Err(io::Error::other("expected flush failure").into()),
    };
    assert!(error.source().is_some_and(|source| source.is::<io::Error>()));
    Ok(())
}
