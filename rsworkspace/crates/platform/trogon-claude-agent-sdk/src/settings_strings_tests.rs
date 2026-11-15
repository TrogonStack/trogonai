#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::borrow::Cow;

use serde_json::{Value, json};

use super::{
    SurrogateStandIns, approximate_strings, exact_string, js_length, python_string, reserve_text, stand_in_escapes,
    take_strings,
};
use crate::{
    constants::SETTINGS_SURROGATE_STAND_IN_RANGES, python_code_point::PythonCodePoint,
    python_json_value::PythonJsonValue, python_string::PythonString,
};

/// A `str` of `units`, each a UTF-16 code unit on its own, so a surrogate stays lone.
fn units(units: &[u16]) -> PythonString {
    let mut text = PythonString::new();
    text.extend(units.iter().copied().map(PythonCodePoint::from));
    text
}

#[test]
fn a_lone_surrogate_has_no_settings_string_outside_a_python_resolve() {
    assert_eq!(python_string(units(&[0xD800])), None);
    assert_eq!(python_string("plain".into()), Some("plain".to_owned()));
}

#[test]
fn a_lone_surrogate_stands_as_one_private_use_code_point_and_turns_back() {
    let _strings = SurrogateStandIns::start();
    let original = units(&[0x61, 0xDC00, 0x62]);
    let stand_in = python_string(original.clone()).expect("a stand-in");
    assert_eq!(stand_in, "a\u{e400}b");
    assert_eq!(stand_in.encode_utf16().count(), 3);
    assert_eq!(exact_string(&stand_in), original);
    assert_eq!(python_string(original.clone()), Some(stand_in));
}

#[test]
fn a_stand_in_moves_past_a_range_the_document_already_holds() {
    let _strings = SurrogateStandIns::start();
    take_strings(&PythonJsonValue::Array(vec![PythonJsonValue::String(
        "\u{e000}".into(),
    )]));
    let stand_in = python_string(units(&[0xD800])).expect("a stand-in");
    assert_eq!(stand_in, "\u{e800}");
    assert_eq!(exact_string("\u{e000}"), PythonString::from("\u{e000}"));
    assert_eq!(exact_string(&stand_in), units(&[0xD800]));
}

#[test]
fn a_typed_value_holds_u_fffd_for_each_lone_surrogate() {
    let _strings = SurrogateStandIns::start();
    let value = python_string(units(&[0xD83D])).expect("a stand-in");
    let key = python_string(units(&[0x6B, 0xDE00])).expect("a stand-in");
    let mut settings = Value::Object([(key, json!([value, "\u{e001}"]))].into_iter().collect());
    approximate_strings(&mut settings);
    assert_eq!(settings, json!({"k\u{fffd}": ["\u{fffd}", "\u{e001}"]}));
}

#[test]
fn a_stand_in_past_the_basic_multilingual_plane_counts_as_one_utf16_unit() {
    let _strings = SurrogateStandIns::start();
    let private_use: String = ('\u{e000}'..='\u{f8ff}').collect();
    reserve_text(&private_use);
    let stand_in = python_string(units(&[0x61, 0xD800])).expect("a stand-in");
    assert!(
        stand_in.chars().nth(1).is_some_and(|c| c >= '\u{f0000}'),
        "{stand_in:?}"
    );
    assert_eq!((js_length(&stand_in), stand_in.encode_utf16().count()), (2, 3));
    assert_eq!(js_length("a\u{f0000}"), if stand_in == "a\u{f0000}" { 2 } else { 3 });
    assert_eq!(js_length(&private_use), 6400);
}

#[test]
fn a_text_ending_in_a_backslash_still_reserves_what_it_holds() {
    let _strings = SurrogateStandIns::start();
    reserve_text("\u{e000}\\");
    assert_eq!(stand_in_escapes(r#""\ud800""#), r#""\ue800""#);
}

#[test]
fn an_escape_without_four_hex_digits_is_copied_as_written() {
    let _strings = SurrogateStandIns::start();
    assert_eq!(
        stand_in_escapes(r#"["\u12zz","\u+123","\ud800"]"#),
        r#"["\u12zz","\u+123","\ue000"]"#
    );
}

#[test]
fn a_lone_surrogate_escape_stays_when_every_private_use_code_point_is_taken() {
    let _strings = SurrogateStandIns::start();
    let private_use: String = SETTINGS_SURROGATE_STAND_IN_RANGES
        .iter()
        .flat_map(|&(first, last)| (first..=last).filter_map(char::from_u32))
        .collect();
    reserve_text(&private_use);
    let text = r#""\ud800""#;
    assert!(matches!(stand_in_escapes(text), Cow::Borrowed(kept) if kept == text));
}
