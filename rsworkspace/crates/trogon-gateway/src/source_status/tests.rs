use super::*;

#[test]
fn default_is_enabled() {
    assert_eq!(SourceStatus::default(), SourceStatus::Enabled);
}

#[test]
fn parses_enabled_and_disabled_case_insensitively() {
    assert_eq!("enabled".parse::<SourceStatus>().unwrap(), SourceStatus::Enabled);
    assert_eq!("DISABLED".parse::<SourceStatus>().unwrap(), SourceStatus::Disabled);
}

#[test]
fn rejects_unknown_values() {
    let err = "maybe".parse::<SourceStatus>().unwrap_err();
    assert_eq!(
        err.to_string(),
        "unsupported status value 'maybe' ; expected 'enabled' or 'disabled'"
    );
}
