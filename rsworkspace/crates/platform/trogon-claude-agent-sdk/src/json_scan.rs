//! The value tokens of JSON text that `serde_json` does not read the way `JSON.parse` or Python's `json.loads`
//! reads them. `serde_json` refuses a number past the double range, where both upstream parsers read an infinity,
//! and refuses Python's `NaN`, `Infinity` and `-Infinity`. A scan respells those tokens so that `serde_json` reads
//! the text, and lists every number and `null` in text order, which is the order `serde_json` visits them in, so
//! a visitor can read each one back from its spelling.

/// A number or `null` value of the text, in text order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Token<'a> {
    /// A number token as written. It may fall outside the JSON grammar; `serde_json` then refuses the text.
    Number(&'a str),
    /// `null`.
    Null,
    /// Python's `NaN`, `Infinity` or `-Infinity`, respelled `null`.
    Constant(f64),
}

/// The text `serde_json` reads in place of the original, and the number and `null` tokens of the original.
pub(crate) struct Scan<'a> {
    pub(crate) text: String,
    pub(crate) tokens: Vec<Token<'a>>,
    /// Whether any token was respelled, so that the original text reads the same through `serde_json`.
    pub(crate) respelled: bool,
}

/// Scans `text`. A number in the JSON grammar past the double range is respelled `0`. With `python_constants`,
/// `NaN`, `Infinity` and `-Infinity` are respelled `null`. A respelled token never joins its neighbors into a
/// different token, so text that is not JSON stays text that is not JSON.
pub(crate) fn scan(text: &str, python_constants: bool) -> Scan<'_> {
    let bytes = text.as_bytes();
    let mut spelled = String::with_capacity(text.len());
    let mut tokens = Vec::new();
    let mut copied = 0;
    let mut index = 0;
    let respell = |start: usize, end: usize, with: &str, spelled: &mut String, copied: &mut usize| {
        spelled.push_str(text.get(*copied..start).unwrap_or_default());
        spelled.push_str(with);
        *copied = end;
    };
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => index = string_end(bytes, index + 1),
            b'-' | b'0'..=b'9' => {
                let start = index;
                index = run_end(bytes, index, |byte| {
                    matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E')
                });
                let token = text.get(start..index).unwrap_or_default();
                let word_end = run_end(bytes, index, |byte| byte.is_ascii_alphabetic());
                if python_constants && token == "-" && text.get(index..word_end) == Some("Infinity") {
                    respell(start, word_end, "null", &mut spelled, &mut copied);
                    tokens.push(Token::Constant(f64::NEG_INFINITY));
                    index = word_end;
                    continue;
                }
                if overflows(token).is_some() {
                    respell(start, index, "0", &mut spelled, &mut copied);
                }
                tokens.push(Token::Number(token));
            }
            byte if byte.is_ascii_alphabetic() => {
                let start = index;
                index = run_end(bytes, index, |byte| byte.is_ascii_alphabetic());
                let constant = match text.get(start..index).unwrap_or_default() {
                    "null" => {
                        tokens.push(Token::Null);
                        continue;
                    }
                    "NaN" if python_constants => f64::NAN,
                    "Infinity" if python_constants => f64::INFINITY,
                    _ => continue,
                };
                respell(start, index, "null", &mut spelled, &mut copied);
                tokens.push(Token::Constant(constant));
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
