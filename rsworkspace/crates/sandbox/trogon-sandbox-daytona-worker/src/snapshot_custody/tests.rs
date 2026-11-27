use super::capture::Capture;
use super::constants::{MAX_CAPTURES, MAX_NAME_LEN};
use super::custody_error::SnapshotCustodyError;
use super::record::{CustodyRecord, validate_name};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "origin-1";
const SOURCE: &str = "sbx-1";

fn record() -> CustodyRecord {
    CustodyRecord::new(ORIGIN, SOURCE, Some("org-1".to_owned()), false)
}

fn bound(id: &str) -> Capture {
    Capture {
        id: Some(id.to_owned()),
        kept: false,
        deleted: false,
        pre_create_absent: true,
        create_acknowledged: true,
    }
}

#[test]
fn a_fresh_record_is_valid() -> TestResult {
    record().validate(ORIGIN, SOURCE)?;
    Ok(())
}

#[test]
fn a_record_for_another_source_or_origin_is_refused() {
    assert!(matches!(
        record().validate(ORIGIN, "sbx-2"),
        Err(SnapshotCustodyError::InvalidIdentity)
    ));
    assert!(matches!(
        record().validate("origin-2", SOURCE),
        Err(SnapshotCustodyError::InvalidIdentity)
    ));
}

#[test]
fn a_record_of_another_version_is_refused() -> TestResult {
    let mut value = serde_json::to_value(record())?;
    value["version"] = serde_json::json!(2);
    let parsed: CustodyRecord = serde_json::from_value(value)?;
    assert!(matches!(
        parsed.validate(ORIGIN, SOURCE),
        Err(SnapshotCustodyError::InvalidIdentity)
    ));
    Ok(())
}

#[test]
fn only_a_closed_record_without_captures_may_lack_an_organization() -> TestResult {
    let fence = CustodyRecord::new(ORIGIN, SOURCE, None, true);
    fence.validate(ORIGIN, SOURCE)?;
    let open = CustodyRecord::new(ORIGIN, SOURCE, None, false);
    assert!(matches!(
        open.validate(ORIGIN, SOURCE),
        Err(SnapshotCustodyError::InvalidIdentity)
    ));
    let mut with_capture = CustodyRecord::new(ORIGIN, SOURCE, None, true);
    with_capture.captures.insert("snap".to_owned(), Capture::intent());
    assert!(matches!(
        with_capture.validate(ORIGIN, SOURCE),
        Err(SnapshotCustodyError::InvalidIdentity)
    ));
    let empty = CustodyRecord::new(ORIGIN, SOURCE, Some(String::new()), false);
    assert!(matches!(
        empty.validate(ORIGIN, SOURCE),
        Err(SnapshotCustodyError::InvalidIdentity)
    ));
    Ok(())
}

#[test]
fn a_record_with_too_many_captures_is_refused() {
    let mut full = record();
    for index in 0..=MAX_CAPTURES {
        full.captures.insert(format!("snap-{index}"), Capture::intent());
    }
    assert!(matches!(
        full.validate(ORIGIN, SOURCE),
        Err(SnapshotCustodyError::InvalidIdentity)
    ));
}

#[test]
fn a_capture_may_be_unbound_bound_kept_or_deleted() -> TestResult {
    let mut value = record();
    value.captures.insert("a".to_owned(), Capture::intent());
    value.captures.insert("b".to_owned(), bound("id-b"));
    value.captures.insert(
        "c".to_owned(),
        Capture {
            kept: true,
            ..bound("id-c")
        },
    );
    value.captures.insert(
        "d".to_owned(),
        Capture {
            deleted: true,
            ..bound("id-d")
        },
    );
    value.validate(ORIGIN, SOURCE)?;
    Ok(())
}

#[test]
fn an_inconsistent_capture_is_refused() {
    let cases = [
        Capture {
            kept: true,
            ..Capture::intent()
        },
        Capture {
            deleted: true,
            ..Capture::intent()
        },
        Capture {
            kept: true,
            deleted: true,
            ..bound("id")
        },
        bound(""),
    ];
    for capture in cases {
        let mut value = record();
        value.captures.insert("snap".to_owned(), capture);
        assert!(matches!(
            value.validate(ORIGIN, SOURCE),
            Err(SnapshotCustodyError::InvalidState)
        ));
    }
}

#[test]
fn a_capture_named_unsafely_is_refused() {
    let mut value = record();
    value.captures.insert("a/b".to_owned(), Capture::intent());
    assert!(matches!(
        value.validate(ORIGIN, SOURCE),
        Err(SnapshotCustodyError::InvalidName)
    ));
}

#[test]
fn names_are_short_path_steps() -> TestResult {
    for good in ["trogon-1", "a_b.c", "x"] {
        validate_name(good)?;
    }
    let too_long = "a".repeat(MAX_NAME_LEN + 1);
    for bad in ["", ".", "..", "a/b", "a b", "a\u{e9}", too_long.as_str()] {
        assert!(
            matches!(validate_name(bad), Err(SnapshotCustodyError::InvalidName)),
            "{bad:?} must be refused"
        );
    }
    validate_name(&"a".repeat(MAX_NAME_LEN))?;
    Ok(())
}

#[test]
fn a_stored_record_with_unknown_fields_is_refused() -> TestResult {
    let mut value = serde_json::to_value(record())?;
    value["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CustodyRecord>(value).is_err());
    Ok(())
}

#[test]
fn a_capture_written_before_the_intent_fields_still_reads() -> TestResult {
    let capture: Capture = serde_json::from_str(r#"{"id":"x","kept":false,"deleted":false}"#)?;
    assert!(!capture.pre_create_absent && !capture.create_acknowledged);
    Ok(())
}
