use super::*;

#[test]
fn rejects_empty_schedule_id() {
    assert!(ScheduleId::parse("").is_err());
}

#[test]
fn accepts_canonical_uuid_v7() {
    let id = crate::queries::ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c13").unwrap();
    assert_eq!(ScheduleId::parse(id.as_str()).unwrap(), id);
}

#[test]
fn from_str_and_as_ref_match_parse() {
    let expected = crate::queries::ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c13").unwrap();
    let id: ScheduleId = expected.as_str().parse().unwrap();
    assert_eq!(id, expected);
    assert_eq!(AsRef::<str>::as_ref(&id), expected.as_str());
    assert!("".parse::<ScheduleId>().is_err());
}

#[test]
fn display_renders_the_raw_id() {
    let id = crate::queries::ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c13").unwrap();
    assert_eq!(id.to_string(), id.as_str());
}

#[test]
fn error_display_and_source_delegate_to_domain() {
    let error = ScheduleId::parse("").unwrap_err();
    assert!(!error.to_string().is_empty());
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn serde_round_trips_through_string() {
    let id = crate::queries::ScheduleId::parse("0198fa2f6d0a7b1a8cf9f762e73a1c13").unwrap();
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{id}\""));
    let decoded: ScheduleId = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, id);
}

#[test]
fn deserialize_rejects_invalid_id() {
    assert!(serde_json::from_str::<ScheduleId>("\"\"").is_err());
}
