use serde_json::Value;
#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use super::{IntError, PythonChar, decimal, from_str, int, is_space, repr};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn goldens() -> Result<Value, serde_json::Error> {
    serde_json::from_str(include_str!("../tests/fixtures/python_text/goldens.json"))
}

/// Every code point gets the digit `unicodedata.decimal` gave it on the CPython that captured the goldens.
#[test]
fn decimal_digits_match_unicodedata() -> TestResult {
    let goldens = goldens()?;
    let mut expected = std::collections::HashMap::new();
    for run in goldens["decimal_runs"].as_array().ok_or("no decimal_runs")? {
        let start = run[0].as_u64().ok_or("no run start")?;
        let length = run[1].as_u64().ok_or("no run length")?;
        for offset in 0..length {
            expected.insert(u32::try_from(start + offset)?, u8::try_from(offset % 10)?);
        }
    }
    assert_eq!(expected.len(), 760);
    for character in (0..=0x10ffff).filter_map(char::from_u32) {
        assert_eq!(
            decimal(character),
            expected.get(&u32::from(character)).copied(),
            "{character:?}"
        );
    }
    Ok(())
}

/// Above ASCII, `int()` turns exactly the characters `str.isspace` accepts into spaces.
#[test]
fn spaces_above_ascii_match_str_isspace() -> TestResult {
    let goldens = goldens()?;
    let expected: Vec<u64> = goldens["non_ascii_spaces"]
        .as_array()
        .ok_or("no non_ascii_spaces")?
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let found: Vec<u64> = (0x80..=0x10ffff)
        .filter_map(char::from_u32)
        .filter(|&character| is_space(character))
        .map(|character| u64::from(u32::from(character)))
        .collect();
    assert_eq!(found, expected);
    Ok(())
}

#[test]
fn int_reads_signs_underscores_and_unicode_digits() {
    let read = |text: &str| int(&from_str(text)).map(|value| value.to_f64());
    assert_eq!(read(" -0_12 "), Ok(Some(-12.0)));
    assert_eq!(read("\u{3000}+\u{661}\u{662}\u{2029}"), Ok(Some(12.0)));
    assert_eq!(read("\u{1d7ce}\u{1d7d7}"), Ok(Some(9.0)));
    assert_eq!(read(&"9".repeat(400)), Ok(None));
    assert_eq!(read(&format!("-{}", "9".repeat(400))), Ok(None));
    assert_eq!(read(&"1".repeat(4301)), Err(IntError::TooManyDigits(4301)));
    for text in ["", " ", "+", "-_1", "1_", "1__0", "1 2", "\u{1c}1", "\u{7f}", "\u{b2}"] {
        assert_eq!(read(text), Err(IntError::InvalidLiteral), "{text:?}");
    }
    assert_eq!(
        int(&[PythonChar::Scalar('1'), PythonChar::Surrogate(0xdc80)]),
        Err(IntError::InvalidLiteral)
    );
}

#[test]
fn repr_escapes_like_python() {
    assert_eq!(repr(&from_str("it's")), "\"it's\"");
    assert_eq!(repr(&from_str("it's \"q\"\\")), "'it\\'s \"q\"\\\\'");
    assert_eq!(
        repr(&from_str("\t\n\r\u{1}\u{7f}\u{a0}\u{e9}")),
        "'\\t\\n\\r\\x01\\x7f\\xa0\u{e9}'"
    );
    assert_eq!(
        repr(&from_str("\u{2028}\u{e000}\u{1f600}\u{e0001}")),
        "'\\u2028\\ue000\u{1f600}\\U000e0001'"
    );
    assert_eq!(
        repr(&[PythonChar::Scalar('\''), PythonChar::Surrogate(0xdcff)]),
        "\"'\\udcff\""
    );
}

#[cfg(unix)]
#[test]
fn os_strings_decode_with_surrogateescape() {
    let text = super::from_os_str(OsStr::from_bytes(b"1\xff\xed\xa0\x80\xc3\xa9\xc3"));
    assert_eq!(repr(&text), "'1\\udcff\\udced\\udca0\\udc80\u{e9}\\udcc3'");
}
