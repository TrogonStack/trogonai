//! The CPython 3.12 text and number helpers `urllib.request.urlopen` and `http.client` lean on: `int()` of a `str`
//! and of `bytes` in base 16, `binascii.a2b_base64` in its default non-strict mode, `urllib.parse.unquote_to_bytes`,
//! the `repr` of `bytes`, the `str` of the `UnicodeEncodeError` the `ascii` and `latin-1` codecs raise, and the `str`
//! of an `OSError`.

use crate::constants::PYTHON_INT_MAX_STR_DIGITS;
use crate::python_re::{decimal_value, is_unicode_space};
use crate::python_string::PythonString;
use std::io;

/// `Py_ISSPACE`, the ASCII whitespace `int()` skips around its digits.
fn is_ascii_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// A Python `int`: its sign and its digits without leading zeros, `0` for zero.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PythonInt {
    negative: bool,
    digits: String,
}

impl PythonInt {
    /// `0`.
    pub(crate) fn zero() -> Self {
        Self {
            negative: false,
            digits: "0".to_owned(),
        }
    }

    /// `str(value)`.
    pub(crate) fn decimal(&self) -> String {
        if self.negative && self.digits != "0" {
            format!("-{}", self.digits)
        } else {
            self.digits.clone()
        }
    }

    /// The value as a C `ssize_t`, `None` when it does not fit.
    pub(crate) fn to_i64(&self) -> Option<i64> {
        let magnitude = self.digits.parse::<u64>().ok()?;
        if self.negative {
            0i64.checked_sub_unsigned(magnitude)
        } else {
            i64::try_from(magnitude).ok()
        }
    }
}

/// `PyLong_FromString` on ASCII text: whitespace, a sign, an optional `0x` prefix in base 16 that one underscore may
/// follow, digits that single underscores may separate, then whitespace. `None` is the `ValueError` it raises.
fn parse_ascii_int(text: &[u8], radix: u32) -> Option<(bool, Vec<u8>)> {
    let start = text
        .iter()
        .position(|&byte| !is_ascii_space(byte))
        .unwrap_or(text.len());
    let end = text
        .iter()
        .rposition(|&byte| !is_ascii_space(byte))
        .map_or(start, |last| last + 1);
    let mut body = text.get(start..end).unwrap_or_default();
    let negative = body.first() == Some(&b'-');
    if matches!(body.first(), Some(b'-' | b'+')) {
        body = body.get(1..).unwrap_or_default();
    }
    if radix == 16 && matches!(body, [b'0', b'x' | b'X', ..]) {
        body = body.get(2..).unwrap_or_default();
        if body.first() == Some(&b'_') {
            body = body.get(1..).unwrap_or_default();
        }
    }
    let mut digits = Vec::with_capacity(body.len());
    let mut previous_underscore = true;
    for &byte in body {
        if byte == b'_' {
            if previous_underscore {
                return None;
            }
            previous_underscore = true;
        } else if char::from(byte).is_digit(radix) {
            digits.push(byte.to_ascii_lowercase());
            previous_underscore = false;
        } else {
            return None;
        }
    }
    if digits.is_empty() || previous_underscore {
        return None;
    }
    Some((negative, digits))
}

/// The `UnicodeDecodeError` text of Python's strict incremental UTF-8 decoder for `bytes`, the held bytes of the
/// last chunk followed by a new one, or how many of its last bytes start a character it holds back.
pub(crate) fn python_decode(bytes: &[u8]) -> Result<usize, String> {
    let error = match std::str::from_utf8(bytes) {
        Ok(_) => return Ok(0),
        Err(error) => error,
    };
    let start = error.valid_up_to();
    let Some(length) = error.error_len() else {
        return Ok(bytes.len() - start);
    };
    let byte = bytes.get(start).copied().unwrap_or_default();
    let reason = if matches!(byte, 0x80..=0xC1 | 0xF5..=0xFF) {
        "invalid start byte"
    } else {
        "invalid continuation byte"
    };
    Err(if length == 1 {
        format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {start}: {reason}")
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {start}-{}: {reason}",
            start + length - 1
        )
    })
}

/// `int(text)` for a `str` in base 10, `None` for the `ValueError` it raises, including the one past
/// `sys.get_int_max_str_digits()`.
pub(crate) fn parse_int(text: &str) -> Option<PythonInt> {
    let mut ascii = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            ascii.push(c as u8);
        } else if is_unicode_space(c) {
            ascii.push(b' ');
        } else {
            ascii.push(b'0' + decimal_value(c)?);
        }
    }
    let (negative, digits) = parse_ascii_int(&ascii, 10)?;
    if digits.len() > PYTHON_INT_MAX_STR_DIGITS {
        return None;
    }
    let digits = String::from_utf8_lossy(&digits);
    let trimmed = digits.trim_start_matches('0');
    Some(PythonInt {
        negative: negative && !trimmed.is_empty(),
        digits: if trimmed.is_empty() {
            "0".to_owned()
        } else {
            trimmed.to_owned()
        },
    })
}

/// A chunk size `http.client` reads, sorted by how `_safe_read(amt)` takes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReadSize {
    /// Fits a C `ssize_t`.
    Bytes(i64),
    /// Past `ssize_t`, which `_safe_read` reads in buffers of at most 1 MiB until the end of the stream.
    Above,
    /// Below `ssize_t`, which `fp.read` refuses with `OverflowError: cannot fit 'int' into an index-sized integer`.
    Below,
}

/// `int(line, 16)` for `bytes`, `None` for the `ValueError` it raises. A NUL byte ends the C string the parser reads,
/// so it is an error like any other stray byte.
pub(crate) fn parse_hex_bytes(line: &[u8]) -> Option<ReadSize> {
    let (negative, digits) = parse_ascii_int(line, 16)?;
    let mut magnitude: u128 = 0;
    for digit in digits {
        let value = char::from(digit).to_digit(16).map_or(0, u128::from);
        magnitude = magnitude.saturating_mul(16).saturating_add(value);
    }
    let value = if negative {
        0i128
            .checked_sub_unsigned(magnitude)
            .and_then(|value| i64::try_from(value).ok())
    } else {
        i64::try_from(magnitude).ok()
    };
    Some(match value {
        Some(value) => ReadSize::Bytes(value),
        None if negative => ReadSize::Below,
        None => ReadSize::Above,
    })
}

/// `base64.decodebytes(data)`, which is `binascii.a2b_base64(data)` with `strict_mode=False`: bytes outside the
/// alphabet are skipped and a pad that completes a quad ends the input. `Err` carries the text of `binascii.Error`.
pub(crate) fn a2b_base64(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 3);
    let mut quad_pos = 0u8;
    let mut left: u8 = 0;
    let mut pads = 0u8;
    for &byte in data {
        if byte == b'=' {
            if quad_pos >= 2 {
                pads += 1;
                if quad_pos + pads >= 4 {
                    return Ok(out);
                }
            }
            continue;
        }
        let Some(value) = base64_value(byte) else {
            continue;
        };
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                left = value;
            }
            1 => {
                quad_pos = 2;
                out.push((left << 2) | (value >> 4));
                left = value & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((left << 4) | (value >> 2));
                left = value & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((left << 6) | value);
            }
        }
    }
    match quad_pos {
        0 => Ok(out),
        1 => Err(format!(
            "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            out.len() / 3 * 4 + 1
        )),
        _ => Err("Incorrect padding".to_owned()),
    }
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// `urllib.parse.unquote_to_bytes(text)`: the UTF-8 of the text with each `%XX` decoded.
pub(crate) fn unquote_to_bytes(text: &str) -> Vec<u8> {
    let mut bits = text.as_bytes().split(|&byte| byte == b'%');
    let mut out = bits.next().unwrap_or_default().to_vec();
    for item in bits {
        match item.get(..2).filter(|hex| hex.iter().all(u8::is_ascii_hexdigit)) {
            Some(hex) => {
                let high = char::from(hex.first().copied().unwrap_or_default())
                    .to_digit(16)
                    .unwrap_or_default();
                let low = char::from(hex.get(1).copied().unwrap_or_default())
                    .to_digit(16)
                    .unwrap_or_default();
                out.push(u8::try_from(high * 16 + low).unwrap_or_default());
                out.extend_from_slice(item.get(2..).unwrap_or_default());
            }
            None => {
                out.push(b'%');
                out.extend_from_slice(item);
            }
        }
    }
    out
}

/// `repr(bytes)`.
pub(crate) fn bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for &byte in bytes {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            byte if char::from(byte) == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(byte)),
            byte => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push(quote);
    out
}

/// One code point as `UnicodeEncodeError` shows it.
fn escaped(code: u32) -> String {
    if code < 0x100 {
        format!("\\x{code:02x}")
    } else if code < 0x10000 {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

/// The `str` of the `UnicodeEncodeError` for the code points `start..end`.
fn encode_error_text(code_points: &[u32], codec: &str, reason: &str, start: usize, end: usize) -> String {
    match code_points.get(start) {
        Some(&code) if end == start + 1 => format!(
            "'{codec}' codec can't encode character '{}' in position {start}: {reason}",
            escaped(code)
        ),
        _ => format!(
            "'{codec}' codec can't encode characters in position {start}-{}: {reason}",
            end.saturating_sub(1)
        ),
    }
}

/// The end of the run of code points from `start` that `fails` refuses.
fn run_end(code_points: &[u32], start: usize, fails: &impl Fn(u32) -> bool) -> usize {
    start
        + code_points
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take_while(|&&code| fails(code))
            .count()
}

/// `text.encode(codec)` for `ascii` (limit 128) and `latin-1` (limit 256): the bytes, or the `str` of the
/// `UnicodeEncodeError` for the first run of characters at or above the limit.
pub(crate) fn encode(text: &str, codec: &str, limit: u32) -> Result<Vec<u8>, String> {
    encode_code_points(&text.chars().map(u32::from).collect::<Vec<_>>(), codec, limit)
}

/// [`encode`] for a Python `str`, whose lone surrogates are at or above either limit.
pub(crate) fn encode_text(text: &PythonString, codec: &str, limit: u32) -> Result<Vec<u8>, String> {
    encode_code_points(
        &text.code_points().map(|code| code.as_u32()).collect::<Vec<_>>(),
        codec,
        limit,
    )
}

fn encode_code_points(code_points: &[u32], codec: &str, limit: u32) -> Result<Vec<u8>, String> {
    let fails = |code: u32| code >= limit;
    match code_points.iter().position(|&code| fails(code)) {
        Some(start) => Err(encode_error_text(
            code_points,
            codec,
            &format!("ordinal not in range({limit})"),
            start,
            run_end(code_points, start, &fails),
        )),
        None => Ok(code_points
            .iter()
            .map(|&code| u8::try_from(code).unwrap_or_default())
            .collect()),
    }
}

/// The `str` of the `UnicodeEncodeError` that encoding `code_points` with the `surrogateescape` error handler
/// raises, if any: in each run of code points the codec refuses, the escapes U+DC80..U+DCFF at its start become
/// bytes, and the rest of the run from the first other code point is the error.
pub(crate) fn surrogateescape_error(
    code_points: &[u32],
    codec: &str,
    reason: &str,
    fails: impl Fn(u32) -> bool,
) -> Option<String> {
    let mut start = 0;
    while let Some(found) = code_points.get(start..)?.iter().position(|&code| fails(code)) {
        let end = run_end(code_points, start + found, &fails);
        let escapes = code_points
            .get(start + found..end)?
            .iter()
            .take_while(|&&code| (0xdc80..=0xdcff).contains(&code))
            .count();
        if start + found + escapes < end {
            return Some(encode_error_text(
                code_points,
                codec,
                reason,
                start + found + escapes,
                end,
            ));
        }
        start = end;
    }
    None
}

/// `latin-1` decoding, which never fails.
pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// The `OSError` subclass CPython raises for an `errno`.
pub(crate) fn os_error_class(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::ConnectionReset => "ConnectionResetError",
        io::ErrorKind::ConnectionAborted => "ConnectionAbortedError",
        io::ErrorKind::ConnectionRefused => "ConnectionRefusedError",
        io::ErrorKind::BrokenPipe => "BrokenPipeError",
        io::ErrorKind::TimedOut => "TimeoutError",
        io::ErrorKind::WouldBlock => "BlockingIOError",
        io::ErrorKind::Interrupted => "InterruptedError",
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::AlreadyExists => "FileExistsError",
        io::ErrorKind::IsADirectory => "IsADirectoryError",
        io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    }
}

/// `strerror(errno)`, the text Rust shows for an OS error without its ` (os error N)` suffix.
pub(crate) fn strerror(code: i32) -> String {
    let text = io::Error::from_raw_os_error(code).to_string();
    let suffix = format!(" (os error {code})");
    text.strip_suffix(&suffix).unwrap_or(&text).to_owned()
}

/// `str(OSError)`: `[Errno N] strerror`, then `: 'filename'` when the error names a file.
pub(crate) fn os_error_text(error: &io::Error, filename: Option<&str>) -> String {
    let base = match error.raw_os_error() {
        Some(code) => format!("[Errno {code}] {}", strerror(code)),
        None => error.to_string(),
    };
    match filename {
        Some(name) => format!("{base}: {}", crate::python_json::python_repr(name)),
        None => base,
    }
}

#[cfg(test)]
#[path = "python_urlopen_text_tests.rs"]
mod tests;
