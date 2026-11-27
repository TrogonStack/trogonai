use serde_json::json;
use trogon_sandbox_contract::{
    Base64Data, LeaseError, LeaseId, TerminalArgument, TerminalArgumentError, TerminalDimension,
    TerminalDimensionError, TerminalError, TerminalId, TerminalIdError, TerminalInput, TerminalInputError, TerminalOp,
    TerminalOutput, TerminalRequest, TerminalResponse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn terminal_id_accepts_valid_ids() -> TestResult {
    let longest = "a".repeat(TerminalId::MAX_LEN);
    for value in ["t", "Ab-9_z", longest.as_str()] {
        let id = value.parse::<TerminalId>()?;
        assert_eq!(id.as_str(), value);
        assert_eq!(id.to_string(), value);
        assert_eq!(TerminalId::try_from(value.to_string())?, id);
        assert_eq!(String::from(id), value);
    }
    Ok(())
}

#[test]
fn terminal_id_rejects_bad_length() -> TestResult {
    let too_long = "a".repeat(TerminalId::MAX_LEN + 1);
    for value in ["", too_long.as_str()] {
        let error = TerminalId::new(value).err().ok_or("expected error")?;
        assert_eq!(error, TerminalIdError::InvalidLength { max: 64 });
        assert_eq!(error.to_string(), "terminal id must be 1 to 64 characters");
    }
    Ok(())
}

#[test]
fn terminal_id_rejects_bad_characters() -> TestResult {
    for value in ["a.b", "a b", "a/b", "caf\u{e9}"] {
        let error = TerminalId::new(value).err().ok_or("expected error")?;
        assert_eq!(error, TerminalIdError::InvalidCharacters);
        assert_eq!(
            error.to_string(),
            "terminal id must contain only ASCII letters, digits, '-' or '_'"
        );
    }
    Ok(())
}

#[test]
fn terminal_id_deserialization_enforces_the_grammar() -> TestResult {
    let id: TerminalId = serde_json::from_value(json!("term-1"))?;
    assert_eq!(serde_json::to_value(&id)?, json!("term-1"));
    let error = serde_json::from_value::<TerminalId>(json!(""))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "terminal id must be 1 to 64 characters");
    let error = serde_json::from_value::<TerminalId>(json!("a b"))
        .err()
        .ok_or("expected error")?;
    assert_eq!(
        error.to_string(),
        "terminal id must contain only ASCII letters, digits, '-' or '_'"
    );
    Ok(())
}

#[test]
fn terminal_dimension_enforces_the_range() -> TestResult {
    for value in [1, 80, TerminalDimension::MAX] {
        let dimension = TerminalDimension::new(value)?;
        assert_eq!(dimension.get(), value);
        assert_eq!(dimension.to_string(), value.to_string());
        assert_eq!(TerminalDimension::try_from(value)?, dimension);
        assert_eq!(u16::from(dimension), value);
    }
    for value in [0, TerminalDimension::MAX + 1] {
        let error = TerminalDimension::new(value).err().ok_or("expected error")?;
        assert_eq!(error, TerminalDimensionError::OutOfRange { max: 1000 });
        assert_eq!(error.to_string(), "terminal dimension must be between 1 and 1000");
        let error = serde_json::from_value::<TerminalDimension>(json!(value))
            .err()
            .ok_or("expected error")?;
        assert_eq!(error.to_string(), "terminal dimension must be between 1 and 1000");
    }
    assert_eq!(serde_json::to_value(TerminalDimension::new(24)?)?, json!(24));
    Ok(())
}

fn request(op: TerminalOp) -> Result<TerminalRequest, Box<dyn std::error::Error>> {
    Ok(TerminalRequest {
        lease_id: LeaseId::new("lease-1"),
        terminal_id: TerminalId::new("term-1")?,
        op,
    })
}

#[test]
fn open_request_round_trips_with_optional_fields() -> TestResult {
    let value = json!({
        "lease_id": "lease-1",
        "terminal_id": "term-1",
        "op": "open",
        "cols": 80,
        "rows": 24,
        "cmd": ["bash", "-l"],
        "env": {"TERM": "xterm"},
        "cwd": "/work",
        "resume_from": 7
    });
    let parsed: TerminalRequest = serde_json::from_value(value.clone())?;
    assert_eq!(
        parsed,
        request(TerminalOp::Open {
            cols: TerminalDimension::new(80)?,
            rows: TerminalDimension::new(24)?,
            cmd: vec![TerminalArgument::new("bash")?, TerminalArgument::new("-l")?],
            env: [("TERM".to_string(), "xterm".to_string())].into(),
            cwd: Some("/work".to_string()),
            resume_from: Some(7),
        })?
    );
    assert_eq!(serde_json::to_value(&parsed)?, value);
    Ok(())
}

#[test]
fn open_request_omits_empty_optional_fields() -> TestResult {
    let value = json!({
        "lease_id": "lease-1",
        "terminal_id": "term-1",
        "op": "open",
        "cols": 80,
        "rows": 24
    });
    let parsed: TerminalRequest = serde_json::from_value(value.clone())?;
    assert_eq!(
        parsed,
        request(TerminalOp::Open {
            cols: TerminalDimension::new(80)?,
            rows: TerminalDimension::new(24)?,
            cmd: Vec::new(),
            env: Default::default(),
            cwd: None,
            resume_from: None,
        })?
    );
    assert_eq!(serde_json::to_value(&parsed)?, value);
    Ok(())
}

#[test]
fn other_ops_round_trip() -> TestResult {
    let cases = [
        (
            json!({"lease_id": "lease-1", "terminal_id": "term-1", "op": "input", "data": "bHM="}),
            TerminalOp::Input {
                data: TerminalInput::new("bHM=")?,
            },
        ),
        (
            json!({"lease_id": "lease-1", "terminal_id": "term-1", "op": "resize", "cols": 100, "rows": 30}),
            TerminalOp::Resize {
                cols: TerminalDimension::new(100)?,
                rows: TerminalDimension::new(30)?,
            },
        ),
        (
            json!({"lease_id": "lease-1", "terminal_id": "term-1", "op": "close"}),
            TerminalOp::Close,
        ),
    ];
    for (value, op) in cases {
        let parsed: TerminalRequest = serde_json::from_value(value.clone())?;
        assert_eq!(parsed, request(op)?);
        assert_eq!(serde_json::to_value(&parsed)?, value);
    }
    Ok(())
}

#[test]
fn request_deserialization_rejects_invalid_values() -> TestResult {
    let error =
        serde_json::from_value::<TerminalRequest>(json!({"lease_id": "lease-1", "terminal_id": "a b", "op": "close"}))
            .err()
            .ok_or("expected error")?;
    assert_eq!(
        error.to_string(),
        "terminal id must contain only ASCII letters, digits, '-' or '_'"
    );
    let error = serde_json::from_value::<TerminalRequest>(
        json!({"lease_id": "lease-1", "terminal_id": "t", "op": "resize", "cols": 0, "rows": 24}),
    )
    .err()
    .ok_or("expected error")?;
    assert_eq!(error.to_string(), "terminal dimension must be between 1 and 1000");
    Ok(())
}

#[test]
fn responses_round_trip() -> TestResult {
    let cases = [
        (
            json!({"outcome": "opened", "terminal_id": "term-1"}),
            TerminalResponse::Opened {
                terminal_id: TerminalId::new("term-1")?,
            },
        ),
        (json!({"outcome": "done"}), TerminalResponse::Done),
        (
            json!({"outcome": "failed", "kind": "too_many_terminals", "max": 8}),
            TerminalResponse::Failed(TerminalError::TooManyTerminals { max: 8 }),
        ),
        (json!({"outcome": "unsupported"}), TerminalResponse::Unsupported),
        (
            json!({"outcome": "denied", "kind": "no_provider_available"}),
            TerminalResponse::Denied(LeaseError::NoProviderAvailable),
        ),
    ];
    for (value, response) in cases {
        let parsed: TerminalResponse = serde_json::from_value(value.clone())?;
        assert_eq!(parsed, response);
        assert_eq!(serde_json::to_value(&parsed)?, value);
    }
    Ok(())
}

#[test]
fn terminal_errors_round_trip_and_display() -> TestResult {
    let cases = [
        (
            json!({"kind": "not_found"}),
            TerminalError::NotFound,
            "terminal not found",
        ),
        (
            json!({"kind": "id_in_use"}),
            TerminalError::IdInUse,
            "terminal id is in use",
        ),
        (
            json!({"kind": "too_many_terminals", "max": 8}),
            TerminalError::TooManyTerminals { max: 8 },
            "the lease already has 8 open terminals",
        ),
        (
            json!({"kind": "invalid_request", "reason": "bad"}),
            TerminalError::InvalidRequest {
                reason: "bad".to_string(),
            },
            "invalid terminal request: bad",
        ),
        (
            json!({"kind": "spawn_failed", "reason": "no shell"}),
            TerminalError::SpawnFailed {
                reason: "no shell".to_string(),
            },
            "terminal could not be started: no shell",
        ),
        (json!({"kind": "exited"}), TerminalError::Exited, "terminal has exited"),
    ];
    for (value, error, display) in cases {
        let parsed: TerminalError = serde_json::from_value(value.clone())?;
        assert_eq!(parsed, error);
        assert_eq!(serde_json::to_value(&parsed)?, value);
        assert_eq!(error.to_string(), display);
    }
    Ok(())
}

#[test]
fn output_frames_round_trip() -> TestResult {
    let cases = [
        (
            json!({"type": "data", "seq": 0, "data": "aGk="}),
            TerminalOutput::Data {
                seq: 0,
                data: Base64Data::new("aGk=")?,
            },
        ),
        (
            json!({"type": "exit", "seq": 3, "exit_code": 1}),
            TerminalOutput::Exit {
                seq: 3,
                exit_code: Some(1),
            },
        ),
        (
            json!({"type": "exit", "seq": 4}),
            TerminalOutput::Exit {
                seq: 4,
                exit_code: None,
            },
        ),
        (
            json!({"type": "resync", "from_seq": 9}),
            TerminalOutput::Resync { from_seq: 9 },
        ),
    ];
    for (value, output) in cases {
        let parsed: TerminalOutput = serde_json::from_value(value.clone())?;
        assert_eq!(parsed, output);
        assert_eq!(serde_json::to_value(&parsed)?, value);
    }
    Ok(())
}

#[test]
fn terminal_argument_rejects_nul() -> TestResult {
    let argument = "ls".parse::<TerminalArgument>()?;
    assert_eq!(argument.as_str(), "ls");
    assert_eq!(argument.to_string(), "ls");
    assert_eq!(TerminalArgument::try_from("ls".to_string())?, argument);
    assert_eq!(String::from(argument), "ls");
    let error = TerminalArgument::new("a\0b").err().ok_or("expected error")?;
    assert_eq!(error, TerminalArgumentError::ContainsNul);
    assert_eq!(error.to_string(), "command arguments must not contain NUL");
    let error = serde_json::from_value::<TerminalArgument>(json!("a\0b"))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "command arguments must not contain NUL");
    let error = serde_json::from_value::<TerminalRequest>(
        json!({"lease_id": "l", "terminal_id": "t", "op": "open", "cols": 1, "rows": 1, "cmd": ["a\0"]}),
    )
    .err()
    .ok_or("expected error")?;
    assert_eq!(error.to_string(), "command arguments must not contain NUL");
    Ok(())
}

fn encode(bytes: usize) -> String {
    let mut text = String::new();
    let full = bytes / 3;
    for _ in 0..full {
        text.push_str("AAAA");
    }
    match bytes % 3 {
        1 => text.push_str("AA=="),
        2 => text.push_str("AAA="),
        _ => {}
    }
    text
}

#[test]
fn terminal_input_accepts_up_to_the_limit() -> TestResult {
    for text in ["", "bHM=", encode(TerminalInput::MAX_BYTES).as_str()] {
        let input = text.parse::<TerminalInput>()?;
        assert_eq!(input.as_str(), text);
        assert_eq!(input.as_base64().as_str(), text);
        assert_eq!(input.to_string(), text);
        assert_eq!(TerminalInput::try_from(text.to_string())?, input);
        assert_eq!(String::from(input), text);
    }
    Ok(())
}

#[test]
fn terminal_input_rejects_oversized_and_invalid_text() -> TestResult {
    let message = "terminal input exceeds 65536 bytes";
    // Decoded size just over the limit, with a valid encoded length.
    let over = encode(TerminalInput::MAX_BYTES + 1);
    // Encoded text longer than any valid encoding of the limit; never decoded.
    let long = "A".repeat(TerminalInput::MAX_BYTES.div_ceil(3) * 4 + 1);
    for text in [over, long] {
        let error = TerminalInput::new(text.as_str()).err().ok_or("expected error")?;
        assert_eq!(error, TerminalInputError::TooLarge { max: 65536 });
        assert_eq!(error.to_string(), message);
        let error = serde_json::from_value::<TerminalInput>(json!(text))
            .err()
            .ok_or("expected error")?;
        assert_eq!(error.to_string(), message);
    }
    let error = TerminalInput::new("***").err().ok_or("expected error")?;
    assert_eq!(error.to_string(), "terminal input is not base64");
    assert!(std::error::Error::source(&error).is_some());
    let error = serde_json::from_value::<TerminalInput>(json!("***"))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "terminal input is not base64");
    Ok(())
}

#[test]
fn invalid_base64_payloads_are_rejected_on_the_wire() -> TestResult {
    let error = serde_json::from_value::<TerminalOutput>(json!({"type": "data", "seq": 0, "data": "***"}))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "value must be standard base64 with padding");
    Ok(())
}
