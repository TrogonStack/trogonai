//! Python `json.dumps` with its defaults: `", "` and `": "` separators, every non-ASCII character escaped as
//! `\uXXXX`, and floats spelled as Python's `repr`.

use std::io::{self, Write};

/// `json.dumps(value)`.
pub(crate) fn to_string<T: serde::Serialize + ?Sized>(value: &T) -> Result<String, serde_json::Error> {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, PythonFormatter);
    value.serialize(&mut serializer)?;
    // The formatter only writes ASCII.
    Ok(String::from_utf8_lossy(&out).into_owned())
}

pub(crate) struct PythonFormatter;

impl serde_json::ser::Formatter for PythonFormatter {
    fn begin_array_value<W: ?Sized + Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        if first { Ok(()) } else { writer.write_all(b", ") }
    }

    fn begin_object_key<W: ?Sized + Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        if first { Ok(()) } else { writer.write_all(b", ") }
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }

    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(float_repr(value).as_bytes())
    }

    fn write_string_fragment<W: ?Sized + Write>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()> {
        for ch in fragment.chars() {
            if (' '..='~').contains(&ch) {
                let mut buf = [0_u8; 4];
                writer.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
            } else {
                let mut units = [0_u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}

/// Python `float.__repr__`: the shortest round-tripping digits, positional when the decimal exponent is in
/// `-4..16`, otherwise scientific with a signed, at least two-digit exponent.
pub(crate) fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or_default();
    let (sign, mantissa) = mantissa.strip_prefix('-').map_or(("", mantissa), |rest| ("-", rest));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let body = if point <= 0 {
            format!("0.{}{digits}", "0".repeat(usize::try_from(-point).unwrap_or_default()))
        } else {
            let point = usize::try_from(point).unwrap_or_default();
            if digits.len() <= point {
                format!("{digits}{}.0", "0".repeat(point - digits.len()))
            } else {
                format!("{}.{}", &digits[..point], &digits[point..])
            }
        };
        format!("{sign}{body}")
    } else {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{first}{fraction}e{exponent_sign}{:02}", exponent.abs())
    }
}

#[cfg(test)]
#[path = "python_json_tests.rs"]
mod tests;
