use trogon_sandbox_windows_fence::{WindowsSandboxErrorCode as Code, parse_launch_error, refusal_beside_others};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_mapped_drive_and_a_seclogon_state_are_kept_on_the_error() -> TestResult {
    let drive = parse_launch_error(r#"{"code":"mapped_drive_cwd","drive":"Z:\\"}"#, "exec").ok_or("no error")?;
    assert_eq!(drive.drive(), Some("Z:\\"));
    let seclogon = parse_launch_error(r#"{"code":"seclogon_disabled","state":"missing"}"#, "exec").ok_or("no error")?;
    assert_eq!(seclogon.state(), Some("missing"));
    Ok(())
}

#[test]
fn profile_busy_maps() -> TestResult {
    let e = refusal_beside_others(&args(&["profile", "grant"]), Some(25), "").ok_or("no refusal")?;
    assert_eq!(e.code(), Code::ProfileOperationInProgress);
    assert_eq!(e.subcommand(), Some("profile"));
    Ok(())
}

#[test]
fn grant_blocked_names_holders() -> TestResult {
    let out = r#"{"held_by":[{"pid":7,"kind":"profile","path":"x"},{"pid":9,"kind":"deny","path":"C:\\d"},{"pid":3,"kind":"odd","path":"p"}]}"#;
    let e = refusal_beside_others(&args(&["profile", "grant"]), Some(24), out).ok_or("no refusal")?;
    assert_eq!(e.code(), Code::ProfileGrantBlockedBySession);
    assert!(e.message().contains(
        "(process 7 is a sandbox session or command; process 9 holds a session deny on C:\\d; process 3 holds odd on p)"
    ));
    Ok(())
}

#[test]
fn held_by_others_names_command_and_caps_list() -> TestResult {
    let rows: Vec<String> = (0..10)
        .map(|i| format!(r#"{{"pid":{i},"kind":"grant","path":"p"}}"#))
        .collect();
    let out = format!(r#"{{"held_by":[{}]}}"#, rows.join(","));
    let e = refusal_beside_others(&args(&["acl", "forget", "x"]), Some(22), &out).ok_or("no refusal")?;
    assert_eq!(e.code(), Code::KeptGrantsInUse);
    assert!(e.message().starts_with("fence-helper acl forget removed nothing"));
    assert!(e.message().contains("; 2 more)"));
    Ok(())
}

#[test]
fn other_statuses_are_not_refusals() {
    for s in [Some(0), Some(1), None] {
        assert!(refusal_beside_others(&args(&["x"]), s, "").is_none());
    }
}

#[test]
fn launch_error_lines() -> TestResult {
    let e = parse_launch_error("noise\n{\"code\":\"seclogon_disabled\",\"state\":\"missing\"}\n", "wfp")
        .ok_or("no error")?;
    assert_eq!(e.code(), Code::SeclogonDisabled);
    assert!(e.message().contains("is not present"));
    assert_eq!(e.subcommand(), Some("wfp"));
    let e = parse_launch_error("{\"code\":\"session_ended\"}", "exec").ok_or("no error")?;
    assert_eq!(e.code(), Code::SessionEnded);
    assert!(parse_launch_error("{\"code\":\"other\"}", "exec").is_none());
    assert!(parse_launch_error("plain", "exec").is_none());
    Ok(())
}

#[test]
fn a_holder_without_a_pid_or_path_is_named_undefined() -> TestResult {
    let out = r#"{"held_by":[{"kind":"profile","path":null}]}"#;
    let e = refusal_beside_others(&args(&["profile", "grant"]), Some(24), out).ok_or("no refusal")?;
    assert!(e.message().contains("process undefined"));
    Ok(())
}

#[test]
fn a_blocked_grant_without_holders_names_none() -> TestResult {
    let e = refusal_beside_others(&args(&["profile", "grant"]), Some(24), "").ok_or("no refusal")?;
    assert_eq!(e.code(), Code::ProfileGrantBlockedBySession);
    assert!(e.message().contains("are running, and the grant"));
    Ok(())
}

#[test]
fn an_error_displays_its_message() -> TestResult {
    let e = refusal_beside_others(&args(&["profile", "grant"]), Some(25), "").ok_or("no refusal")?;
    assert_eq!(e.to_string(), e.message());
    Ok(())
}
