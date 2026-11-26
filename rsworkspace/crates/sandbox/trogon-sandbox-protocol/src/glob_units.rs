pub(crate) const DASH: u16 = b'-' as u16;
pub(crate) const SLASH: u16 = b'/' as u16;
pub(crate) const STAR_UNIT: u16 = b'*' as u16;
pub(crate) const OPEN: u16 = b'[' as u16;
pub(crate) const CLOSE: u16 = b']' as u16;
pub(crate) const QUESTION: u16 = b'?' as u16;

/// The UTF-16 code units of `text`, which is how the glob dialect counts characters.
pub(crate) fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

pub(crate) fn text_of(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// The lowercase of a code unit, or the unit itself when it has no single-unit lowercase.
pub(crate) fn fold_unit(unit: u16) -> u16 {
    char::from_u32(u32::from(unit))
        .and_then(|c| c.to_lowercase().next())
        .and_then(|c| u16::try_from(u32::from(c)).ok())
        .unwrap_or(unit)
}
