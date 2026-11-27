use super::*;

fn fed(lines: &[&str]) -> Collected {
    let mut collected = Collected::with_limit(64);
    for line in lines {
        collected.line(line.as_bytes());
    }
    collected
}

#[test]
fn stream_records_fill_the_matching_buffer() {
    let collected = fed(&[
        r#"{"stream":"stdout","data":"out"}"#,
        r#"{"stream":"stderr","data":"err"}"#,
        r#"{"stream":"stdout","data":"put"}"#,
    ]);
    assert_eq!(collected.stdout.into_bytes(), b"output");
    assert_eq!(collected.stderr.into_bytes(), b"err");
    assert!(!collected.truncated);
}

#[test]
fn output_past_the_limit_marks_the_capture_truncated() {
    let collected = fed(&[&format!(r#"{{"stream":"stdout","data":"{}"}}"#, "x".repeat(200))]);
    assert!(collected.truncated);
}

#[test]
fn an_error_record_keeps_a_bounded_message() {
    assert_eq!(
        fed(&[r#"{"stream":"error","data":{"code":"c","message":"gone"}}"#])
            .error
            .as_deref(),
        Some("gone")
    );
    assert_eq!(
        fed(&[r#"{"stream":"error","data":"plain"}"#]).error.as_deref(),
        Some("plain")
    );
    assert_eq!(
        fed(&[r#"{"stream":"error"}"#]).error.as_deref(),
        Some("command stream error")
    );
    let long = format!(r#"{{"stream":"error","data":"{}"}}"#, "m".repeat(500));
    assert_eq!(fed(&[&long]).error.map(|message| message.len()), Some(300));
}

#[test]
fn command_records_carry_the_id_and_the_exit_code() {
    let collected = fed(&[
        r#"{"command":{"id":"cmd_1","exitCode":null}}"#,
        r#"{"command":{"id":"cmd_1","exitCode":7}}"#,
    ]);
    assert_eq!(collected.command_id.as_deref(), Some("cmd_1"));
    assert_eq!(collected.exit, Some(7));
    assert_eq!(fed(&[r#"{"command":{"exitCode":99999999999}}"#]).exit, Some(-1));
    assert_eq!(fed(&[r#"{"command":{}}"#]).command_id, None);
}

#[test]
fn records_that_carry_nothing_are_ignored() {
    let collected = fed(&[
        "not json",
        r#"{"stream":"stdout","data":5}"#,
        r#"{"stream":"stdout"}"#,
        r#"{"stream":"other","data":"x"}"#,
        r#"{"unrelated":true}"#,
    ]);
    assert!(collected.stdout.into_bytes().is_empty());
    assert!(collected.stderr.into_bytes().is_empty());
    assert!(collected.error.is_none() && collected.command_id.is_none() && collected.exit.is_none());
}
