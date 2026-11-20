#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::parse_policy_text;
use crate::{ResolveSettingsError, SETTINGS_DEPTH_BOUND};

fn nested(depth: usize) -> String {
    format!("{}null{}", "{\"x\":".repeat(depth - 1), "}".repeat(depth - 1))
}

#[test]
fn a_document_that_parses_to_something_other_than_an_object_is_not_an_object() {
    assert!(matches!(parse_policy_text("[]"), Ok(None)));
    assert!(matches!(parse_policy_text("\u{feff}\"text\""), Ok(None)));
    assert!(matches!(parse_policy_text("{"), Ok(None)));
    let arrays = format!("{}{}", "[".repeat(200), "]".repeat(200));
    assert!(matches!(parse_policy_text(&arrays), Ok(None)));
}

#[test]
fn a_document_nested_past_serde_json_that_json_parse_rejects_is_not_an_object() {
    assert!(matches!(parse_policy_text(&"[".repeat(200)), Ok(None)));
}

#[test]
fn a_document_nested_past_serde_json_reads_as_its_object() {
    let document = parse_policy_text(&nested(200)).expect("reads").expect("an object");
    assert!(document.keys().eq(["x"]));
}

#[test]
fn a_document_one_level_below_the_bound_reads_as_its_object() {
    let document = parse_policy_text(&nested(SETTINGS_DEPTH_BOUND - 1)).expect("reads");
    assert!(document.is_some());
}

#[test]
fn an_object_document_at_the_bound_fails_with_the_maximum_call_stack() {
    assert!(matches!(
        parse_policy_text(&nested(SETTINGS_DEPTH_BOUND)),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
}

#[test]
fn an_array_document_at_the_bound_is_not_an_object() {
    let arrays = format!(
        "{}{}",
        "[".repeat(SETTINGS_DEPTH_BOUND),
        "]".repeat(SETTINGS_DEPTH_BOUND)
    );
    assert!(matches!(parse_policy_text(&arrays), Ok(None)));
}
