use super::*;

const ID: &str = "0198fa2f6d0a7b1a8cf9f762e73a1c45";

#[test]
fn accepts_and_normalizes_uuid_text() {
    for (raw, expected) in [
        (ID, ID),
        ("0198fa2f-6d0a-7b1a-8cf9-f762e73a1c45", ID),
        ("0198FA2F-6D0A-7B1A-8CF9-F762E73A1C45", ID),
        ("0198fa2f6d0a4b1a7cf9f762e73a1c45", "0198fa2f6d0a4b1a7cf9f762e73a1c45"),
    ] {
        assert_eq!(ScheduleId::parse(raw).unwrap().to_string(), expected);
    }
}

#[test]
fn rejects_non_uuid_text() {
    for raw in ["", "backup", "0198fa2f6d0a7b1a8cf9f762e73a1c4g"] {
        let error = ScheduleId::parse(raw).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("schedule id '{raw}' is invalid: must be a UUID")
        );
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let id = ScheduleId::parse(ID).unwrap();
    let uuid = Uuid::parse_str(ID).unwrap();

    assert_eq!(*id.as_uuid(), uuid);
    assert_eq!(ScheduleId::from(uuid), id);
    assert_eq!(ID.parse::<ScheduleId>().unwrap().to_string(), ID);
    assert_eq!(id.to_string(), ID);
    assert_eq!(
        ScheduleId::parse("").unwrap_err().to_string(),
        "schedule id '' is invalid: must be a UUID"
    );
}

mod prop_tests;
