use super::*;

#[test]
fn rejects_empty_schedule_id() {
    assert!(ScheduleId::parse("").is_err());
}

#[test]
fn accepts_ids_a_single_nats_token_would_reject() {
    for raw in ["report.v2", "orders/created", "ns:thing", "café-nightly"] {
        assert_eq!(ScheduleId::parse(raw).unwrap().as_str(), raw, "{raw:?}");
    }
}

#[test]
fn from_str_and_as_ref_match_parse() {
    let id: ScheduleId = "orders/created".parse().unwrap();
    assert_eq!(id.as_str(), "orders/created");
    assert_eq!(AsRef::<str>::as_ref(&id), "orders/created");
    assert!("".parse::<ScheduleId>().is_err());
}

#[test]
fn display_renders_the_raw_id() {
    let id = ScheduleId::parse("orders/created").unwrap();
    assert_eq!(id.to_string(), "orders/created");
}

#[test]
fn error_display_and_source_delegate_to_domain() {
    let error = ScheduleId::parse("").unwrap_err();
    assert!(!error.to_string().is_empty());
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn serde_round_trips_through_string() {
    let id = ScheduleId::parse("orders/created").unwrap();
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"orders/created\"");
    let decoded: ScheduleId = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, id);
}

#[test]
fn deserialize_rejects_invalid_id() {
    assert!(serde_json::from_str::<ScheduleId>("\"\"").is_err());
}
