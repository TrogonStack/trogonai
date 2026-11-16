//! The value tokens of JSON text that `serde_json` does not read the way `JSON.parse` reads them. `serde_json`
//! refuses a number past the double range, where `JSON.parse` reads an infinity. A scan respells those tokens so
//! that `serde_json` reads the text, and lists every number and `null` in text order, which is the order
//! `serde_json` visits them in, so a visitor can read each one back from its spelling.

/// A number or `null` value of the text, in text order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Token<'a> {
    /// A number token as written. It may fall outside the JSON grammar; `serde_json` then refuses the text.
    Number(&'a str),
    /// `null`.
    Null,
}

/// The text `serde_json` reads in place of the original, and the number and `null` tokens of the original.
pub(crate) struct Scan<'a> {
    pub(crate) text: String,
    pub(crate) tokens: Vec<Token<'a>>,
    /// Whether any token was respelled, so that the original text reads the same through `serde_json`.
    pub(crate) respelled: bool,
}

/// Scans `text`. A number in the JSON grammar past the double range is respelled `0`, which never joins its
/// neighbors into a different token, so text that is not JSON stays text that is not JSON.
pub(crate) fn scan(text: &str) -> Scan<'_> {
    let bytes = text.as_bytes();
    let mut spelled = String::with_capacity(text.len());
    let mut tokens = Vec::new();
    let mut copied = 0;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => index = string_end(bytes, index + 1),
            b'-' | b'0'..=b'9' => {
                let start = index;
                index = run_end(bytes, index, |byte| {
                    matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E')
                });
                let token = text.get(start..index).unwrap_or_default();
                if overflows(token).is_some() {
                    spelled.push_str(text.get(copied..start).unwrap_or_default());
                    spelled.push('0');
                    copied = index;
                }
                tokens.push(Token::Number(token));
            }
            byte if byte.is_ascii_alphabetic() => {
                let start = index;
                index = run_end(bytes, index, |byte| byte.is_ascii_alphabetic());
                if text.get(start..index) == Some("null") {
                    tokens.push(Token::Null);
                }
            }
            _ => index += 1,
        }
    }
    let respelled = copied > 0;
    spelled.push_str(text.get(copied..).unwrap_or_default());
    Scan {
        text: spelled,
        tokens,
        respelled,
    }
}

/// `Some(negative)` when `token` is a number in the JSON grammar that reads past the double range.
pub(crate) fn overflows(token: &str) -> Option<bool> {
    Some(token)
        .filter(|token| is_json_number(token))
        .and_then(|token| token.parse::<f64>().ok())
        .filter(|value| value.is_infinite())
        .map(f64::is_sign_negative)
}

/// Whether `token` is a number in the JSON grammar.
pub(crate) fn is_json_number(token: &str) -> bool {
    let unsigned = token.strip_prefix('-').unwrap_or(token);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (integer, fraction) = match mantissa.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (mantissa, None),
    };
    (integer == "0" || (all_digits(integer) && !integer.starts_with('0')))
        && fraction.is_none_or(all_digits)
        && exponent.is_none_or(|exponent| all_digits(exponent.strip_prefix(['+', '-']).unwrap_or(exponent)))
}

fn all_digits(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
}

fn run_end(bytes: &[u8], mut index: usize, accepts: impl Fn(u8) -> bool) -> usize {
    while bytes.get(index).is_some_and(|&byte| accepts(byte)) {
        index += 1;
    }
    index
}

/// The index just past the closing quote of the string whose contents start at `index`.
fn string_end(bytes: &[u8], mut index: usize) -> usize {
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
#[path = "json_scan_tests.rs"]
mod tests;
