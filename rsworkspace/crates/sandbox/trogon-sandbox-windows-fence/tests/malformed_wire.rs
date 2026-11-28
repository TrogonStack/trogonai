use trogon_sandbox_windows_fence::{WindowsSandboxErrorCode as Code, parse_profile_read_status, parse_sandbox_status};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const USER: &str = r#""user":{"user":{"exists":true,"group_exists":true,"in_builtin_users":false,
  "in_sandbox_group":true,"hidden_from_logon":true},"cred_present":true,"real_user_sid":"S-1"#;
const WFP: &str = r#""wfp":{"state":"installed","filters":6}"#;

fn status(extra: &str) -> String {
    format!(r#"{{{USER}"{extra}}},{WFP}}}"#)
}

fn refused(result: Result<impl std::fmt::Debug, trogon_sandbox_windows_fence::WindowsSandboxError>) -> TestResult {
    let error = result.err().ok_or("accepted")?;
    assert_eq!(error.code(), Code::FenceHelperBadJson);
    Ok(())
}

#[test]
fn an_unknown_seclogon_state_is_refused() -> TestResult {
    refused(parse_sandbox_status(
        &status(r#","seclogon":"bogus""#),
        &["status".to_string()],
    ))?;
    parse_sandbox_status(&status(r#","seclogon":"ok""#), &["status".to_string()])?;
    parse_sandbox_status(&status(""), &["status".to_string()])?;
    Ok(())
}

#[test]
fn malformed_capabilities_are_refused() -> TestResult {
    let args = ["status".to_string()];
    refused(parse_sandbox_status(&status(r#","capabilities":["a",5]"#), &args))?;
    refused(parse_sandbox_status(&status(r#","capabilities":"a""#), &args))?;
    Ok(())
}

#[test]
fn malformed_profile_status_lists_are_refused() -> TestResult {
    for body in [
        r#"{"state":"none","capabilities":["a",3]}"#,
        r#"{"state":"none","not_granted":[{"reason":"no path"}]}"#,
        r#"{"state":"none","stale_allow":"not a list"}"#,
        r#"{"state":"none","replaced_files":"x"}"#,
        r#"{"state":"none","extra_exclusions":[1]}"#,
        r#"{"state":"none","paths":[{"path":"a","kind":"odd"}]}"#,
        r#"{"state":"none","paths":[{"kind":"file"}]}"#,
    ] {
        refused(parse_profile_read_status(body))?;
    }
    parse_profile_read_status(r#"{"state":"none","not_granted":null}"#)?;
    Ok(())
}
