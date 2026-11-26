use super::{PythonCodePoint, PythonCodePointError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn every_number_up_to_unicode_is_a_code_point() -> TestResult {
    assert_eq!(PythonCodePoint::try_from(0x10_FFFF_u32)?.as_u32(), 0x10_FFFF);
    assert_eq!(
        PythonCodePoint::try_from(0xD800_u32)?,
        PythonCodePoint::from(0xD800_u16)
    );
    assert_eq!(
        PythonCodePoint::try_from(0x11_0000_u32),
        Err(PythonCodePointError::OutOfRange(0x11_0000))
    );
    Ok(())
}

#[test]
fn a_surrogate_is_no_char() {
    assert_eq!(PythonCodePoint::from(0xDFFF_u16).as_char(), None);
    assert_eq!(PythonCodePoint::from('\u{e9}').as_char(), Some('\u{e9}'));
}

#[test]
fn surrogates_are_told_apart_by_their_range() {
    let classes: Vec<(bool, bool)> = [0xD7FF_u16, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xE000]
        .map(PythonCodePoint::from)
        .iter()
        .map(|code_point| (code_point.is_high_surrogate(), code_point.is_low_surrogate()))
        .collect();
    assert_eq!(
        classes,
        [
            (false, false),
            (true, false),
            (true, false),
            (false, true),
            (false, true),
            (false, false)
        ]
    );
}

#[test]
fn a_surrogate_pair_joins_as_utf16_decodes_it() {
    let pair = |high: u16, low: u16| PythonCodePoint::from_surrogates(high.into(), low.into());
    assert_eq!(pair(0xD83D, 0xDE00), PythonCodePoint::from('\u{1F600}'));
    assert_eq!(pair(0xDBFF, 0xDFFF).as_u32(), 0x10_FFFF);
    assert_eq!(pair(0xD800, 0xDC00).as_u32(), 0x1_0000);
}
