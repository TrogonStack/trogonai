//! Vendor passthrough option helpers.

use serde_json::{Map, Value, json};
use trogon_sandbox_contract::{ProviderDispatchReply, ProviderKind};
use trogon_sandbox_protocol::{
    NativeOptionsError, ProviderError, dispatch_failure_reply, optional_bool, optional_choice, optional_u64,
    options_for, reject_unknown_keys,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object(value: &Value) -> Result<&Map<String, Value>, Box<dyn std::error::Error>> {
    Ok(value.as_object().ok_or("object")?)
}

#[test]
fn absent_native_yields_no_options() -> TestResult {
    assert!(options_for(ProviderKind::Daytona, None)?.is_empty());
    Ok(())
}

#[test]
fn picks_the_entry_under_the_provider_token() -> TestResult {
    let native = json!({"daytona": {"public": true}});
    let options = options_for(ProviderKind::Daytona, Some(&native))?;
    assert_eq!(options.get("public"), Some(&json!(true)));
    Ok(())
}

#[test]
fn a_missing_provider_entry_yields_no_options() -> TestResult {
    let native = json!({});
    assert!(options_for(ProviderKind::Daytona, Some(&native))?.is_empty());
    Ok(())
}

#[test]
fn options_for_another_provider_are_refused() -> TestResult {
    let native = json!({"local_docker": {"pids_limit": 10}});
    let error = options_for(ProviderKind::Daytona, Some(&native))
        .err()
        .ok_or("expected an error")?;
    assert!(error.reason().contains("local_docker"), "{}", error.reason());
    Ok(())
}

#[test]
fn non_object_payloads_are_refused() {
    assert!(options_for(ProviderKind::Daytona, Some(&json!([1]))).is_err());
    assert!(options_for(ProviderKind::Daytona, Some(&json!({"daytona": 3}))).is_err());
}

#[test]
fn unknown_keys_are_named_in_the_error() -> TestResult {
    let options = json!({"public": true, "bogus": 1});
    let error = reject_unknown_keys(object(&options)?, &["public"])
        .err()
        .ok_or("expected an error")?;
    assert!(error.reason().contains("'bogus'"), "{}", error.reason());
    assert!(error.reason().contains("public"), "{}", error.reason());
    reject_unknown_keys(object(&json!({"public": true}))?, &["public"])?;
    Ok(())
}

#[test]
fn integer_options_are_range_checked() -> TestResult {
    let options = json!({"n": 5, "big": 500, "text": "x"});
    let map = object(&options)?;
    assert_eq!(optional_u64(map, "n", 1, 10)?, Some(5));
    assert_eq!(optional_u64(map, "missing", 1, 10)?, None);
    assert!(optional_u64(map, "big", 1, 10).is_err());
    assert!(optional_u64(map, "text", 1, 10).is_err());
    Ok(())
}

#[test]
fn choice_options_must_be_one_of_the_allowed_values() -> TestResult {
    let options = json!({"mode": "fast", "bad": "slow", "num": 1});
    let map = object(&options)?;
    assert_eq!(
        optional_choice(map, "mode", &["fast", "safe"])?,
        Some("fast".to_string())
    );
    assert_eq!(optional_choice(map, "missing", &["fast"])?, None);
    let error = optional_choice(map, "bad", &["fast", "safe"])
        .err()
        .ok_or("expected an error")?;
    assert!(error.reason().contains("fast, safe"));
    assert!(optional_choice(map, "num", &["fast"]).is_err());
    Ok(())
}

#[test]
fn boolean_options_must_be_booleans() -> TestResult {
    let options = json!({"on": true, "text": "yes"});
    let map = object(&options)?;
    assert_eq!(optional_bool(map, "on")?, Some(true));
    assert_eq!(optional_bool(map, "missing")?, None);
    assert!(optional_bool(map, "text").is_err());
    Ok(())
}

#[test]
fn failure_reply_distinguishes_native_errors() -> TestResult {
    let native = ProviderError::NativeOptions(NativeOptionsError::new("nope"));
    assert!(matches!(
        dispatch_failure_reply(&native),
        ProviderDispatchReply::NativeRejected { native_error } if native_error == "nope"
    ));
    let other = ProviderError::backend(std::io::Error::other("boom"));
    assert!(matches!(
        dispatch_failure_reply(&other),
        ProviderDispatchReply::LaunchFailed { provider_error } if provider_error.contains("boom")
    ));
    Ok(())
}
