use serde_json::json;

use super::*;

#[test]
fn no_native_payload_is_the_default() -> Result<(), NativeOptionsError> {
    assert_eq!(native_params(None)?, NativeParams::default());
    Ok(())
}

#[test]
fn every_option_is_read() -> Result<(), NativeOptionsError> {
    let native = json!({"daytona": {
        "auto_stop_interval": 5,
        "auto_archive_interval": 6,
        "auto_delete_interval": 7,
        "public": true,
    }});
    let params = native_params(Some(&native))?;
    assert_eq!(params.auto_stop_interval, Some(5));
    assert_eq!(params.auto_archive_interval, Some(6));
    assert_eq!(params.auto_delete_interval, Some(7));
    assert_eq!(params.public, Some(true));
    Ok(())
}

#[test]
fn unknown_keys_and_bad_values_are_refused() {
    for native in [
        json!({"daytona": {"gpu": 1}}),
        json!({"daytona": {"auto_stop_interval": "soon"}}),
        json!({"daytona": {"auto_stop_interval": MAX_INTERVAL_MINUTES + 1}}),
        json!({"daytona": {"public": 1}}),
    ] {
        assert!(native_params(Some(&native)).is_err(), "{native}");
    }
}
