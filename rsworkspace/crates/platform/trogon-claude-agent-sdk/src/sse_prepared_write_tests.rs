use super::*;

fn stamped(line: &str) -> Result<String, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    Ok(SsePreparedWrite::with_uuid(line, value.as_object(), || "u".to_owned()))
}

#[test]
fn an_object_without_a_uuid_gets_one() -> Result<(), serde_json::Error> {
    assert_eq!(stamped(r#"{"type":"user"}"#)?, r#"{"type":"user","uuid":"u"}"#);
    assert_eq!(stamped(r#"{"uuid":"","a":1}"#)?, r#"{"uuid":"u","a":1}"#);
    assert_eq!(stamped(r#"{"uuid":5}"#)?, r#"{"uuid":"u"}"#);
    Ok(())
}

#[test]
fn other_lines_are_unchanged() -> Result<(), serde_json::Error> {
    assert_eq!(stamped(r#"{"uuid":"given"} "#)?, r#"{"uuid":"given"} "#);
    assert_eq!(stamped("[1, 2]")?, "[1, 2]");
    assert_eq!(stamped("5")?, "5");
    Ok(())
}

#[test]
fn the_body_wraps_the_line() -> Result<(), serde_json::Error> {
    let write = SsePreparedWrite {
        line: r#"{"a":1}"#.to_owned(),
        log: None,
    };
    assert_eq!(
        write.post_body("s1")?,
        r#"{"session_id":"s1","events":[{"payload":{"a":1}}]}"#
    );
    let broken = SsePreparedWrite {
        line: "nope".to_owned(),
        log: None,
    };
    assert!(broken.post_body("s1").is_err());
    Ok(())
}
