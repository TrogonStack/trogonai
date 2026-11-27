use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NONCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const EXECUTION: &str = "0123456789abcdef0123456789abcdef";

fn parse(value: serde_json::Value) -> Result<OrdinaryState, serde_json::Error> {
    serde_json::from_value(value)
}

fn running() -> serde_json::Value {
    serde_json::json!({
        "version": 1, "execution_id": EXECUTION, "nonce": NONCE, "phase": "running",
        "helper": {"pid": 10, "start_time": 1}, "root": {"pid": 11, "start_time": 2},
        "root_exit": null, "completion": null,
    })
}

fn finished() -> serde_json::Value {
    let digest = "ab".repeat(32);
    let mut value = running();
    value["phase"] = "absent".into();
    value["root_exit"] = serde_json::json!({"exit_code": 0, "signal": null});
    value["completion"] = serde_json::json!({
        "released": true, "root_exit_code": 0, "root_signal": null, "timed_out": false, "canceled": false,
        "reaped_count": 1, "stdout_truncated": false, "stderr_truncated": false, "stdout_len": 0, "stderr_len": 0,
        "stdout_sha256": digest, "stderr_sha256": digest,
    });
    value
}

#[test]
fn a_running_state_names_both_identities() -> TestResult {
    let identities = parse(running())?.validate(EXECUTION, NONCE)?;
    assert_eq!(
        identities,
        Some((
            OrdinaryIdentity { pid: 10, start_time: 1 },
            OrdinaryIdentity { pid: 11, start_time: 2 }
        ))
    );
    Ok(())
}

#[test]
fn a_prepared_state_has_no_identities_yet() -> TestResult {
    let mut value = running();
    value["phase"] = "prepared".into();
    value["helper"] = serde_json::Value::Null;
    value["root"] = serde_json::Value::Null;
    assert_eq!(parse(value)?.validate(EXECUTION, NONCE)?, None);
    Ok(())
}

#[test]
fn a_state_for_another_execution_or_nonce_is_refused() -> TestResult {
    assert!(parse(running())?.validate("other", NONCE).is_err());
    assert!(parse(running())?.validate(EXECUTION, &"f".repeat(64)).is_err());
    Ok(())
}

#[test]
fn identities_must_be_distinct_and_not_init() -> TestResult {
    for (helper, root) in [(1, 11), (10, 10), (10, 1)] {
        let mut value = running();
        value["helper"]["pid"] = helper.into();
        value["root"]["pid"] = root.into();
        assert!(parse(value)?.validate(EXECUTION, NONCE).is_err());
    }
    Ok(())
}

#[test]
fn a_running_state_without_identities_is_refused() -> TestResult {
    let mut value = running();
    value["helper"] = serde_json::Value::Null;
    value["root"] = serde_json::Value::Null;
    assert!(parse(value)?.validate(EXECUTION, NONCE).is_err());
    Ok(())
}

#[test]
fn an_absent_state_needs_a_consistent_completion() -> TestResult {
    assert!(parse(finished())?.validate(EXECUTION, NONCE)?.is_some());
    let mut missing = finished();
    missing["completion"] = serde_json::Value::Null;
    assert!(parse(missing)?.validate(EXECUTION, NONCE).is_err());
    let mut mismatched = finished();
    mismatched["root_exit"] = serde_json::json!({"exit_code": 1, "signal": null});
    assert!(parse(mismatched)?.validate(EXECUTION, NONCE).is_err());
    let mut unreaped = finished();
    unreaped["completion"]["reaped_count"] = 0.into();
    assert!(parse(unreaped)?.validate(EXECUTION, NONCE).is_err());
    let mut unreleased = finished();
    unreleased["completion"]["released"] = false.into();
    assert!(parse(unreleased)?.validate(EXECUTION, NONCE).is_err());
    let mut bad_digest = finished();
    bad_digest["completion"]["stdout_sha256"] = "XYZ".into();
    assert!(parse(bad_digest)?.validate(EXECUTION, NONCE).is_err());
    Ok(())
}

#[test]
fn a_deadline_without_a_cancelation_is_inconsistent() -> TestResult {
    let mut value = finished();
    value["completion"]["timed_out"] = true.into();
    assert!(parse(value)?.validate(EXECUTION, NONCE).is_err());
    Ok(())
}

#[test]
fn a_completion_before_the_end_is_refused() -> TestResult {
    let mut value = finished();
    value["phase"] = "running".into();
    assert!(parse(value)?.validate(EXECUTION, NONCE).is_err());
    Ok(())
}

#[test]
fn unknown_fields_and_phases_do_not_parse() {
    let mut extra = running();
    extra["surprise"] = 1.into();
    assert!(parse(extra).is_err());
    let mut phase = running();
    phase["phase"] = "dancing".into();
    assert!(parse(phase).is_err());
}
