use super::*;

#[test]
fn rejects_empty() {
    let error = ScheduleId::parse("").unwrap_err();
    assert_eq!(error.violation, ScheduleIdViolation::Empty);
}

#[test]
fn rejects_surrounding_whitespace() {
    for raw in [" backup", "backup ", "\tbackup", "backup\n"] {
        let error = ScheduleId::parse(raw).unwrap_err();
        assert_eq!(error.violation, ScheduleIdViolation::SurroundingWhitespace, "{raw:?}");
    }
}

#[test]
fn accepts_max_length() {
    let raw = "a".repeat(MAX_LENGTH);
    assert_eq!(ScheduleId::parse(&raw).unwrap().as_str(), raw);
}

#[test]
fn rejects_over_max_length() {
    let raw = "a".repeat(MAX_LENGTH + 1);
    let error = ScheduleId::parse(&raw).unwrap_err();
    assert_eq!(
        error.violation,
        ScheduleIdViolation::TooLong {
            max: MAX_LENGTH,
            actual: MAX_LENGTH + 1,
        }
    );
}

#[test]
fn counts_length_in_characters_not_bytes() {
    let raw = "é".repeat(MAX_LENGTH);
    assert_eq!(ScheduleId::parse(&raw).unwrap().as_str(), raw);
}

#[test]
fn accepts_values_a_nats_token_would_reject() {
    for raw in ["report.v2", "orders/created", "ns:thing", "user@host", "a.b.c.d"] {
        assert_eq!(ScheduleId::parse(raw).unwrap().as_str(), raw, "{raw:?}");
    }
}

#[test]
fn accepts_non_ascii() {
    for raw in ["café-nightly", "日次バックアップ", "Pública"] {
        assert_eq!(ScheduleId::parse(raw).unwrap().as_str(), raw, "{raw:?}");
    }
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let id = ScheduleId::parse("backup").unwrap();

    assert_eq!(id.as_ref(), "backup");
    assert_eq!("backup".parse::<ScheduleId>().unwrap().as_str(), "backup");
    assert_eq!(id.to_string(), "backup");
    assert_eq!(ScheduleIdViolation::Empty.to_string(), "must not be empty");
    assert_eq!(
        ScheduleIdViolation::TooLong { max: 256, actual: 257 }.to_string(),
        "must be at most 256 characters, got 257"
    );
    assert_eq!(
        ScheduleIdViolation::SurroundingWhitespace.to_string(),
        "must not have leading or trailing whitespace"
    );
    assert_eq!(
        ScheduleId::parse("").unwrap_err().to_string(),
        "schedule id '' is invalid: must not be empty"
    );
}

mod prop_tests;
