#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;

#[test]
fn interrupt_keeps_legacy_absence_distinct() -> Result<(), serde_json::Error> {
    assert!(matches!(
        InterruptReceipt::from_json(JsonObject::new()),
        InterruptReceipt::NotReported { .. }
    ));
    let value = serde_json::json!({"still_queued":[],"cancelled":[]});
    let value: JsonObject = serde_json::from_value(value)?;
    assert!(
        matches!(InterruptReceipt::from_json(value), InterruptReceipt::Reported { still_queued, .. } if still_queued.is_empty())
    );
    Ok(())
}

#[test]
fn control_replies_compare_clone_and_hand_back_the_raw_response() -> Result<(), serde_json::Error> {
    let reply: ControlReply<Vec<String>> = ControlReply::new(Some(serde_json::json!(["a"])));
    let copy = reply.clone();
    assert_eq!(copy, reply);
    assert_ne!(ControlReply::<Vec<String>>::new(Some(serde_json::Value::Null)), reply);
    assert_eq!(format!("{reply:?}"), r#"ControlReply(Some(Array [String("a")]))"#);
    assert_eq!(reply.decode()?, vec!["a".to_owned()]);
    assert_eq!(reply.into_json(), Some(serde_json::json!(["a"])));
    Ok(())
}

#[test]
fn an_absent_response_stays_distinct_from_null_like_typescript_undefined() {
    let absent = ControlReply::<Option<Vec<String>>>::new(None);
    let null = ControlReply::<Option<Vec<String>>>::new(Some(serde_json::Value::Null));
    assert_ne!(absent, null);
    assert_eq!(absent.as_json(), None);
    assert_eq!(null.as_json(), Some(&serde_json::Value::Null));
    assert!(matches!(absent.decode(), Ok(None)));
    assert_eq!(absent.into_json(), None);
    assert_eq!(ConnectionInfo::from_response(None).as_json(), None);
    assert_eq!(
        ConnectionInfo::from_response(Some(serde_json::Value::Null)).as_json(),
        Some(&serde_json::Value::Null)
    );
}

#[test]
fn control_option_values_are_the_upstream_literals() {
    assert_eq!(McpPermissionModeOverride::Default.as_str(), "default");
    assert_eq!(McpPermissionModeOverride::Auto.as_str(), "auto");
    assert_eq!(MaxThinkingTokensDisplay::Summarized.as_str(), "summarized");
    assert_eq!(MaxThinkingTokensDisplay::Omitted.as_str(), "omitted");
    assert_eq!(MaxThinkingTokensDisplay::Highlights.as_str(), "highlights");
    assert_eq!(SettingsUpdateSource::LocalSettings.as_str(), "localSettings");
    assert_eq!(SettingsUpdateSource::UserSettings.as_str(), "userSettings");
    assert_eq!(ContextUsageDetail::Summary.as_str(), "summary");
    assert_eq!(ContextUsageDetail::Full.as_str(), "full");
    assert_eq!(ReadFileEncoding::Utf8.as_str(), "utf-8");
    assert_eq!(ReadFileEncoding::Base64.as_str(), "base64");
}
