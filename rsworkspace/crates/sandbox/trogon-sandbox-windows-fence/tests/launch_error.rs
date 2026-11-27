use trogon_sandbox_windows_fence::{WindowsSandboxErrorCode as Code, parse_launch_error};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn codes_without_a_message_get_a_default_one() -> TestResult {
    let cases = [
        (
            "profile_operation_in_progress",
            Code::ProfileOperationInProgress,
            "in progress",
        ),
        ("session_ended", Code::SessionEnded, "ended"),
        ("mapped_drive_cwd", Code::MappedDriveCwd, "network-drive"),
        ("wfp_loopback_mismatch", Code::WfpLoopbackMismatch, "loopback mode"),
    ];
    for (name, code, text) in cases {
        let line = format!(r#"{{"code":"{name}"}}"#);
        let error = parse_launch_error(&line, "exec").ok_or("no error")?;
        assert_eq!(error.code(), code);
        assert!(error.message().contains(text), "{name}: {}", error.message());
    }
    Ok(())
}

#[test]
fn a_message_from_the_helper_is_kept() -> TestResult {
    for name in [
        "profile_operation_in_progress",
        "session_ended",
        "wfp_loopback_mismatch",
    ] {
        let line = format!(r#"{{"code":"{name}","message":"said so"}}"#);
        let error = parse_launch_error(&line, "exec").ok_or("no error")?;
        assert_eq!(error.message(), "said so");
    }
    Ok(())
}

#[test]
fn lines_that_are_not_coded_json_are_skipped() -> TestResult {
    let stderr = "plain text\n{not json \"code\"\n{\"other\":1}\n{\"code\":\"session_ended\"}\n";
    let error = parse_launch_error(stderr, "exec").ok_or("no error")?;
    assert_eq!(error.code(), Code::SessionEnded);
    assert!(parse_launch_error("nothing here", "exec").is_none());
    Ok(())
}
