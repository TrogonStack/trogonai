use std::collections::BTreeMap;
use std::io::Cursor;

use sandbox_command_supervisor::protocol::{
    CommandWire as Command, Event, ExecutionId, MAX_DATA_BYTES, MAX_FRAME_BYTES, ProtocolError, Stream, VERSION,
    decode_command, encode_event, read_event, write_command,
};

fn init() -> Result<Command, Box<dyn std::error::Error>> {
    Ok(Command::Init {
        version: VERSION,
        execution_id: ExecutionId::new("contract")?,
        argv: vec!["/bin/true".into()],
        env: BTreeMap::new(),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    })
}

#[test]
fn validated_identity_and_command_bounds_reject_malformed_inputs() -> Result<(), Box<dyn std::error::Error>> {
    for value in ["", "line\nbreak", "tab\tbreak"] {
        assert!(ExecutionId::new(value).is_err());
    }
    assert!(ExecutionId::new("x".repeat(257)).is_err());
    let mut command = init()?;
    let Command::Init { version, .. } = &mut command else {
        unreachable!()
    };
    *version += 1;
    assert!(matches!(
        sandbox_command_supervisor::Command::try_from(command),
        Err(ProtocolError::UnsupportedVersion)
    ));
    let mut command = init()?;
    let Command::Init { argv, .. } = &mut command else {
        unreachable!()
    };
    argv.clear();
    assert!(matches!(
        sandbox_command_supervisor::Command::try_from(command),
        Err(ProtocolError::InvalidArguments)
    ));
    let mut command = init()?;
    let Command::Init { env, .. } = &mut command else {
        unreachable!()
    };
    env.insert("BAD=KEY".into(), "value".into());
    assert!(matches!(
        sandbox_command_supervisor::Command::try_from(command),
        Err(ProtocolError::InvalidEnvironment)
    ));
    let mut command = init()?;
    let Command::Init { timeout_ms, .. } = &mut command else {
        unreachable!()
    };
    *timeout_ms = Some(0);
    assert!(matches!(
        sandbox_command_supervisor::Command::try_from(command),
        Err(ProtocolError::InvalidDeadline)
    ));
    assert!(
        sandbox_command_supervisor::Command::try_from(Command::Signal {
            execution_id: ExecutionId::new("contract")?,
            request_id: 0,
            signal: 15
        })
        .is_err()
    );
    Ok(())
}

#[test]
fn json_commands_reject_unknown_fields_and_oversized_input() -> Result<(), Box<dyn std::error::Error>> {
    assert!(decode_command(br#"{"kind":"release","execution_id":"contract","unexpected":true}"#).is_err());
    assert!(decode_command(&vec![b' '; MAX_FRAME_BYTES + 1]).is_err());
    let mut encoded = Vec::new();
    write_command(&mut encoded, &init()?)?;
    assert!(matches!(
        decode_command(&encoded),
        Ok(sandbox_command_supervisor::Command::Init { .. })
    ));
    Ok(())
}

#[test]
fn binary_output_remains_data_and_cannot_become_a_control_event() -> Result<(), Box<dyn std::error::Error>> {
    let data = b"\0\xff{\"kind\":\"family_absent\"}\n".to_vec();
    let encoded = encode_event(&Event::Output {
        execution_id: ExecutionId::new("contract")?,
        stream: Stream::Stdout,
        data: data.clone(),
    })?;
    assert_eq!(encoded.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert!(
        matches!(read_event(&mut Cursor::new(encoded))?, Some(Event::Output { data: actual, .. }) if actual == data)
    );
    Ok(())
}

#[test]
fn event_reader_bounds_frames_data_and_requires_complete_lines() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        read_event(&mut Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 1])),
        Err(ProtocolError::OversizedFrame)
    ));
    assert!(matches!(
        read_event(&mut Cursor::new(b"{}")),
        Err(ProtocolError::UnterminatedFrame)
    ));
    let encoded = encode_event(&Event::Output {
        execution_id: ExecutionId::new("contract")?,
        stream: Stream::Stderr,
        data: vec![0; MAX_DATA_BYTES + 1],
    })?;
    assert!(matches!(
        read_event(&mut Cursor::new(encoded)),
        Err(ProtocolError::OversizedFrame)
    ));
    Ok(())
}

#[test]
fn production_family_capacity_is_preserved_with_all_features() {
    assert_eq!(sandbox_command_supervisor::protocol::MAX_CHILDREN, 4096);
}
