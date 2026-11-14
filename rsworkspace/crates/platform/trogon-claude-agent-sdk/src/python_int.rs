//! Python's `int(text)` for the environment values the Python dialect reads.

pub(crate) enum PythonInt {
    Negative,
    /// Saturated at `u64::MAX`, which is longer than any wait.
    Positive(u64),
}

/// The values Python's `int(text)` accepts in base 10: surrounding whitespace, one sign, and ASCII digits with
/// single underscores between them.
pub(crate) fn python_int(text: &str) -> Option<PythonInt> {
    let text = text.trim();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut value = 0_u64;
    for byte in digits.bytes() {
        match byte {
            b'_' => {}
            b'0'..=b'9' => value = value.saturating_mul(10).saturating_add(u64::from(byte - b'0')),
            _ => return None,
        }
    }
    Some(if negative && value > 0 {
        PythonInt::Negative
    } else {
        PythonInt::Positive(value)
    })
}
