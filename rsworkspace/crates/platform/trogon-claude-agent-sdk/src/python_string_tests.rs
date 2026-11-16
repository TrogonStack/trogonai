use super::PythonString;
use crate::python_code_point::PythonCodePoint;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn string_of(code_points: &[u32]) -> Result<PythonString, Box<dyn std::error::Error>> {
    let mut string = PythonString::new();
    for code_point in code_points {
        string.push(PythonCodePoint::try_from(*code_point)?);
    }
    Ok(string)
}

#[test]
fn code_points_of_every_width_round_trip() -> TestResult {
    let code_points = [
        0x41, 0x7F, 0x80, 0x7FF, 0x800, 0xD7FF, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xE000, 0xFFFF, 0x1_0000, 0x10_FFFF,
    ];
    let string = string_of(&code_points)?;
    let read: Vec<u32> = string.code_points().map(PythonCodePoint::as_u32).collect();
    assert_eq!(read, code_points);
    Ok(())
}

#[test]
fn a_surrogate_pair_stays_two_code_points() -> TestResult {
    let pair = string_of(&[0xD83D, 0xDE00])?;
    assert_ne!(pair, PythonString::from("\u{1F600}"));
    assert_eq!(pair.code_points().count(), 2);
    assert_eq!(pair.clone().into_string(), Err(pair));
    Ok(())
}

#[test]
fn text_without_a_surrogate_is_a_string() {
    let mut string = PythonString::from("caf");
    string.extend("\u{e9} \u{1F600}".chars().map(PythonCodePoint::from));
    assert_eq!(string, PythonString::from("caf\u{e9} \u{1F600}"));
    assert_eq!(string.into_string(), Ok(String::from("caf\u{e9} \u{1F600}")));
}

#[test]
fn debug_quotes_like_a_str_and_spells_out_surrogates() {
    let mut string = PythonString::from("it's \"\n");
    string.push(PythonCodePoint::from(0xD800_u16));
    assert_eq!(format!("{string:?}"), "\"it's \\\"\\n\\u{d800}\"");
}

#[test]
fn a_str_finds_the_key_holding_the_same_text() -> TestResult {
    let mut keys = indexmap::IndexMap::new();
    keys.insert(PythonString::from(String::from("caf\u{e9}")), 1);
    keys.insert(string_of(&[0x61, 0xD800])?, 2);
    assert_eq!(
        (keys.get("caf\u{e9}"), keys.get("a"), keys.get("a\u{fffd}")),
        (Some(&1), None, None)
    );
    assert_eq!(keys.get(&string_of(&[0x61, 0xD800])?), Some(&2));
    assert_eq!(
        (PythonString::from("x").as_str(), string_of(&[0xDC00])?.as_str()),
        (Some("x"), None)
    );
    Ok(())
}

#[test]
fn text_pushed_after_a_surrogate_follows_it() -> TestResult {
    let mut string = string_of(&[0xD800])?;
    string.push_str("a\u{e9}");
    assert_eq!(string, string_of(&[0xD800, 0x61, 0xE9])?);
    Ok(())
}

#[test]
fn surrogateescape_turns_each_byte_outside_utf8_into_a_surrogate() -> TestResult {
    assert_eq!(
        PythonString::decode_surrogateescape(b"a\xff\xc3\xa9\xed\xa0\x80b\xf0\x9f\x98"),
        string_of(&[0x61, 0xDCFF, 0xE9, 0xDCED, 0xDCA0, 0xDC80, 0x62, 0xDCF0, 0xDC9F, 0xDC98])?
    );
    Ok(())
}

#[test]
fn mapped_text_keeps_its_surrogates_and_cases_as_python() -> TestResult {
    let lower = string_of(&[0x41, 0x3A3, 0xDCFF, 0x3A3])?.map_text(crate::python_urllib::python_lower);
    assert_eq!(lower, string_of(&[0x61, 0x3C2, 0xDCFF, 0x3C3])?);
    Ok(())
}

#[test]
fn stripping_never_removes_a_surrogate() -> TestResult {
    let text = string_of(&[0x20, 0xA0, 0xDCFF, 0x20, 0x78, 0xA0, 0x20])?;
    assert_eq!(
        text.trim_matches(crate::python_json::is_python_whitespace),
        string_of(&[0xDCFF, 0x20, 0x78])?
    );
    Ok(())
}

#[test]
fn encoding_a_surrogate_fails_as_python_reports_it() -> TestResult {
    assert_eq!(PythonString::from("caf\u{e9}").encode_utf8(), Ok("caf\u{e9}"));
    let cases = [
        (
            string_of(&[0x75, 0x73, 0xDCFF, 0x65, 0x72])?,
            "'utf-8' codec can't encode character '\\udcff' in position 2: surrogates not allowed",
        ),
        (
            string_of(&[0x70, 0xDCFE, 0xDCFD, 0x77])?,
            "'utf-8' codec can't encode characters in position 1-2: surrogates not allowed",
        ),
        (
            string_of(&[0xDC80])?,
            "'utf-8' codec can't encode character '\\udc80' in position 0: surrogates not allowed",
        ),
    ];
    for (text, error) in cases {
        assert_eq!(text.encode_utf8(), Err(error.to_owned()));
    }
    Ok(())
}

#[test]
fn lossy_text_has_one_replacement_per_surrogate() -> TestResult {
    assert_eq!(
        string_of(&[0x61, 0xD800, 0x62, 0xDCFF])?.to_str_lossy(),
        "a\u{fffd}b\u{fffd}"
    );
    Ok(())
}
