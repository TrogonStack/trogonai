use serde_json::json;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_reply_becomes_a_grant_with_the_expiry_in_unix_seconds() -> TestResult {
    let grant: DaytonaSshGrant = serde_json::from_value(json!({
        "id": "a1",
        "sandboxId": "sbx-1",
        "token": "tok123",
        "expiresAt": "2026-01-01T00:00:00Z",
        "sshCommand": "ssh -p 2222 tok123@gateway.test"
    }))?;
    assert_eq!(grant.host, "gateway.test");
    assert_eq!(grant.port, 2222);
    assert_eq!(grant.token, "tok123");
    assert_eq!(grant.command, "ssh -p 2222 tok123@gateway.test");
    assert_eq!(grant.expires_at, Some(1_767_225_600));
    Ok(())
}

#[test]
fn the_token_field_stands_in_when_the_command_names_no_user() -> TestResult {
    let grant: DaytonaSshGrant = serde_json::from_value(json!({
        "token": "tok123",
        "sshCommand": "ssh gateway.test"
    }))?;
    assert_eq!(grant.token, "tok123");
    assert_eq!(grant.port, 22);
    assert_eq!(grant.expires_at, None);
    Ok(())
}

#[test]
fn a_command_without_a_host_is_refused() {
    let grant = serde_json::from_value::<DaytonaSshGrant>(json!({
        "token": "tok123",
        "sshCommand": "ssh tok123@"
    }));
    assert!(grant.is_err());
}

#[test]
fn an_expiry_that_is_not_a_timestamp_is_refused() {
    let grant = serde_json::from_value::<DaytonaSshGrant>(json!({
        "token": "tok123",
        "expiresAt": "soon",
        "sshCommand": "ssh tok123@gateway.test"
    }));
    assert!(grant.is_err());
}

#[test]
fn debug_hides_the_token_and_the_command() -> TestResult {
    let grant: DaytonaSshGrant = serde_json::from_value(json!({
        "token": "tok123",
        "sshCommand": "ssh tok123@gateway.test"
    }))?;
    let shown = format!("{grant:?}");
    assert!(!shown.contains("tok123"));
    assert!(shown.contains("gateway.test"));
    Ok(())
}
