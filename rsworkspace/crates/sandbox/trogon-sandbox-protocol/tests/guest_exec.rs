mod common;

use base64::Engine;
use common::TestResult;
use trogon_sandbox_contract::{CommandTimeoutStatus, LeaseError, OutputTruncation, ReadFileResponse};
use trogon_sandbox_protocol::{
    FileNotFoundError, GUEST_STREAM_LIMIT, GuestCaptureError, ReadFileError, parse_guest_capture, read_file_response,
};

fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn denied(message: String) -> LeaseError {
    LeaseError::Backend { message }
}

#[test]
fn small_output_is_returned_whole() -> TestResult {
    let reply = format!("7\n2\n{}\n\n3\n{}\n\n", b64(&[0, 255]), b64(b"err"));
    let out = parse_guest_capture(&reply, 5, 1024)?;
    assert_eq!(out.exit_code, 7);
    assert_eq!(out.stdout, [0, 255]);
    assert_eq!(out.stderr, b"err");
    assert_eq!(out.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(out.truncated, OutputTruncation::Complete);
    Ok(())
}

#[test]
fn a_timeout_marker_is_reported_as_a_timeout_with_exit_minus_one() -> TestResult {
    let reply = format!("timeout\n3\n{}\n\n0\n\n\n", b64(b"out"));
    let out = parse_guest_capture(&reply, 9, 1024)?;
    assert_eq!(out.exit_code, -1);
    assert_eq!(out.timed_out, CommandTimeoutStatus::TimedOut);
    assert_eq!(out.stdout, b"out");
    assert_eq!(out.stderr, b"command timed out after 9s");
    Ok(())
}

#[test]
fn a_command_that_exits_124_itself_is_not_a_timeout() -> TestResult {
    let reply = format!("124\n3\n{}\n\n0\n\n\n", b64(b"out"));
    let out = parse_guest_capture(&reply, 9, 1024)?;
    assert_eq!(out.exit_code, 124);
    assert_eq!(out.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(out.stdout, b"out");
    Ok(())
}

#[test]
fn a_guest_side_gap_is_marked_truncated() -> TestResult {
    let reply = format!("0\n100\n{}\n{}\n0\n\n\n", b64(b"head"), b64(b"tail"));
    let out = parse_guest_capture(&reply, 5, 1024)?;
    assert_eq!(out.truncated, OutputTruncation::Truncated);
    assert_eq!(
        String::from_utf8(out.stdout)?,
        "head\n[... 92 bytes truncated ...]\ntail"
    );
    Ok(())
}

#[test]
fn output_over_the_request_budget_is_capped_head_and_tail() -> TestResult {
    let big = vec![b'x'; 1000];
    let reply = format!("0\n1000\n{}\n\n0\n\n\n", b64(&big));
    let out = parse_guest_capture(&reply, 5, 100)?;
    assert_eq!(out.truncated, OutputTruncation::Truncated);
    assert!(out.stdout.len() < 200);
    Ok(())
}

#[test]
fn malformed_replies_are_typed_errors() -> TestResult {
    let cases = [
        ("", GuestCaptureError::InvalidExitCode),
        ("x\n", GuestCaptureError::InvalidExitCode),
        ("0", GuestCaptureError::MissingStreamSize),
        ("0\nzz\n", GuestCaptureError::InvalidStreamSize),
        ("0\n1\nAAAA\n\n0\n\n\n", GuestCaptureError::MoreThanHeld),
        ("0\n1\n\n\n", GuestCaptureError::InvalidStreamSize),
    ];
    for (reply, expected) in cases {
        let error = parse_guest_capture(reply, 5, 10).err().ok_or("expected error")?;
        assert_eq!(error, expected, "{reply:?}");
    }
    let error = parse_guest_capture("0\n1\n!!!\n\n0\n\n\n", 5, 10)
        .err()
        .ok_or("expected error")?;
    assert!(matches!(error, GuestCaptureError::Encoding(_)));
    assert!(!error.to_string().is_empty());
    for error in [
        GuestCaptureError::MissingExitCode,
        GuestCaptureError::InvalidExitCode,
        GuestCaptureError::MissingStreamSize,
        GuestCaptureError::InvalidStreamSize,
        GuestCaptureError::MoreThanHeld,
    ] {
        assert!(!error.to_string().is_empty());
    }
    Ok(())
}

#[test]
fn a_missing_file_maps_to_not_found_and_other_failures_to_denied() {
    let missing: ReadFileError<String> = ReadFileError::NotFound(FileNotFoundError { path: "/a".into() });
    assert_eq!(read_file_response(Err(missing), denied), ReadFileResponse::NotFound);
    let boom: ReadFileError<String> = ReadFileError::Other("boom".to_string());
    assert!(matches!(
        read_file_response(Err(boom), denied),
        ReadFileResponse::Denied(_)
    ));
    assert_eq!(
        read_file_response::<String>(Ok(b"data".to_vec()), denied),
        ReadFileResponse::Content { data: b"data".to_vec() }
    );
    assert_eq!(
        FileNotFoundError { path: "/a".into() }.to_string(),
        "file not found: /a"
    );
}

#[test]
fn the_stream_limit_is_sixteen_mebibytes() {
    assert_eq!(GUEST_STREAM_LIMIT, 16 * 1024 * 1024);
}
