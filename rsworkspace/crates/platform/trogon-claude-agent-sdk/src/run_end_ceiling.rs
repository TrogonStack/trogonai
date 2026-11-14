//! `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`: how long after the last result a run waits for an idle state.
//!
//! The TypeScript parse is `gd` and `wC` from `sdk.mjs` 0.3.323 followed by its `j7` fallback and the
//! `armRunEndCeiling` clamp. The Python parse is `run_end_ceiling_ms` from `claude-agent-sdk` 0.2.181, whose `int()`
//! accepts Unicode decimal digits and refuses more than 4300 of them, followed by its `min(ms, 2**31 - 1)` sleep.
//! Both give the applied ceiling: `None` is no limit.

use std::{ffi::OsStr, time::Duration};

use crate::{ClaudeAgentOptions, RunEndTimeout, adapters::EnvironmentVariable, process_options::EnvironmentMode};

const VARIABLE: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";
const DEFAULT_MILLISECONDS: u64 = 600_000;
/// JavaScript's `setTimeout` limit and Python's `2**31 - 1`.
const MAXIMUM_MILLISECONDS: u64 = 2_147_483_647;
/// Python's default `sys.get_int_max_str_digits()`.
const PYTHON_MAXIMUM_DIGITS: usize = 4300;
/// `wC` only looks at strings of at most this many UTF-16 code units.
const JAVASCRIPT_MAXIMUM_UNITS: usize = 32;

/// The first code point of every run of ten consecutive decimal digits in Unicode 15, which Python 3.12 reads.
const DECIMAL_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66, 0xde6, 0xe50, 0xed0,
    0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620,
    0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0,
    0x112f0, 0x11450, 0x114d0, 0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50,
    0x16a60, 0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950,
    0x1fbf0,
];

/// The value the child process would see for the variable, as the selected dialect reads it.
pub(crate) fn resolve<E: EnvironmentVariable>(options: &ClaudeAgentOptions, environment: &E) -> Option<Duration> {
    let configured = options.env.get(VARIABLE).map(|value| value.expose().to_owned());
    let inherited = || {
        environment
            .var_os(OsStr::new(VARIABLE))
            .map(|value| value.to_string_lossy().into_owned())
    };
    let raw = match options.limits.run_end_timeout {
        // TypeScript reads the environment it builds for the child: `options.env` alone when it replaces the
        // inherited one, the inherited one when there is no `options.env`.
        RunEndTimeout::TypeScript => match options.environment_mode.resolve(&options.env) {
            EnvironmentMode::Replace => configured,
            EnvironmentMode::Inherit => inherited(),
            EnvironmentMode::Overlay | EnvironmentMode::TypeScript => configured.or_else(inherited),
        },
        // Python reads `options.env` first and falls back to `os.environ`.
        RunEndTimeout::Python | RunEndTimeout::Fixed(_) => configured.or_else(inherited),
    };
    options.limits.run_end_timeout.for_variable(raw.as_deref())
}

fn applied(milliseconds: u64) -> Option<Duration> {
    (milliseconds > 0).then(|| Duration::from_millis(milliseconds.min(MAXIMUM_MILLISECONDS)))
}

/// `j7` and `armRunEndCeiling`: anything that is not a finite number of at least zero is the default, zero arms no
/// timer, and the rest is clamped to `setTimeout`'s limit.
pub(crate) fn typescript(raw: Option<&str>) -> Option<Duration> {
    let value = raw.map_or(f64::NAN, javascript_number);
    if !(value.is_finite() && value >= 0.0) {
        return applied(DEFAULT_MILLISECONDS);
    }
    applied(value.min(f64::from(i32::MAX)) as u64)
}

/// `run_end_ceiling_ms` and the sleeper's clamp: anything `int()` refuses or a negative number is the default, zero
/// starts no sleeper.
pub(crate) fn python(raw: Option<&str>) -> Option<Duration> {
    applied(raw.and_then(python_int).unwrap_or(DEFAULT_MILLISECONDS))
}

/// `gd`: the trimmed text through `wC`, then `parseInt(text, 10)`. NaN stands for both NaN and a missing value.
fn javascript_number(raw: &str) -> f64 {
    let text = raw.trim_matches(is_javascript_space);
    spelled_number(text).unwrap_or_else(|| parse_int(text))
}

/// `String.prototype.trim`: white space and line terminators.
fn is_javascript_space(character: char) -> bool {
    matches!(
        character,
        '\t'..='\r'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// `wC`: short text in exponent form is a number that must be whole, and short text with one separator between
/// groups of three digits is those digits. Anything else is left to `parseInt`.
fn spelled_number(text: &str) -> Option<f64> {
    if text.encode_utf16().count() > JAVASCRIPT_MAXIMUM_UNITS {
        return None;
    }
    if is_exponent_form(text) {
        let whole = text
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite() && number.fract() == 0.0);
        return Some(whole.unwrap_or(f64::NAN));
    }
    grouped_digits(text)
}

/// `/^[+-]?(\d+(\.\d*)?|\.\d+)[eE][+-]?\d+$/`
fn is_exponent_form(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |from: usize| bytes[from..].iter().take_while(|byte| byte.is_ascii_digit()).count();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let whole = digits(index);
    index += whole;
    if whole > 0 {
        if bytes.get(index) == Some(&b'.') {
            index += 1;
            index += digits(index);
        }
    } else {
        if bytes.get(index) != Some(&b'.') {
            return false;
        }
        index += 1;
        let fraction = digits(index);
        if fraction == 0 {
            return false;
        }
        index += fraction;
    }
    if !matches!(bytes.get(index), Some(b'e' | b'E')) {
        return false;
    }
    index += 1;
    index += usize::from(matches!(bytes.get(index), Some(b'+' | b'-')));
    let exponent = digits(index);
    exponent > 0 && index + exponent == bytes.len()
}

/// `/^[+-]?\d{1,3}([_,   ])\d{3}(?:\1\d{3})*$/` followed by `parseInt` of the digits.
fn grouped_digits(text: &str) -> Option<f64> {
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let separator = unsigned
        .chars()
        .find(|character| matches!(character, '_' | ',' | '\u{a0}' | '\u{202f}' | ' '))?;
    let groups: Vec<&str> = unsigned.split(separator).collect();
    let is_digits = |group: &&str| group.bytes().all(|byte| byte.is_ascii_digit());
    let head_fits = groups.first().is_some_and(|head| (1..=3).contains(&head.len()));
    let tail_fits = groups.iter().skip(1).all(|group| group.len() == 3);
    if groups.len() < 2 || !head_fits || !tail_fits || !groups.iter().all(is_digits) {
        return None;
    }
    let magnitude = groups.concat().parse::<f64>().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// `parseInt(text, 10)`: an optional sign and the leading ASCII digits, NaN without any.
fn parse_int(text: &str) -> f64 {
    let (negative, rest) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let length = rest.bytes().take_while(u8::is_ascii_digit).count();
    // A run of digits always parses; one past the largest double is infinity.
    let magnitude = rest[..length].parse::<f64>().unwrap_or(f64::NAN);
    if negative { -magnitude } else { magnitude }
}

/// Python's `int(text)` in base 10 for a non-negative result, saturated at `u64::MAX`. Negative numbers, text `int()`
/// refuses and more than 4300 digits are `None`.
fn python_int(text: &str) -> Option<u64> {
    let text = text.trim_matches(is_python_space);
    let (negative, body) = match text.chars().next() {
        Some('-') => (true, &text[1..]),
        Some('+') => (false, &text[1..]),
        _ => (false, text),
    };
    let mut value = 0_u64;
    let mut digits = 0_usize;
    // No underscore may open the digits, follow another underscore or close them.
    let mut after_underscore = true;
    for character in body.chars() {
        if character == '_' {
            if after_underscore {
                return None;
            }
            after_underscore = true;
            continue;
        }
        let digit = decimal_digit(character)?;
        after_underscore = false;
        digits += 1;
        value = value.saturating_mul(10).saturating_add(u64::from(digit));
    }
    if after_underscore || digits > PYTHON_MAXIMUM_DIGITS {
        return None;
    }
    (!negative || value == 0).then_some(value)
}

/// What `int()` strips: ASCII white space, and the Unicode white space CPython rewrites to a plain space first.
fn is_python_space(character: char) -> bool {
    matches!(
        character,
        '\t'..='\r'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// The value of a Unicode decimal digit, as CPython's `Py_UNICODE_TODECIMAL` reads it.
fn decimal_digit(character: char) -> Option<u8> {
    let code = u32::from(character);
    let block = DECIMAL_ZEROS.partition_point(|zero| *zero <= code).checked_sub(1)?;
    u8::try_from(code - DECIMAL_ZEROS[block])
        .ok()
        .filter(|digit| *digit <= 9)
}

#[cfg(test)]
#[path = "run_end_ceiling_tests.rs"]
mod tests;
