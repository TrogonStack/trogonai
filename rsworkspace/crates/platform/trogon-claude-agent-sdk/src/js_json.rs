//! TypeScript `JSON.stringify`: serde_json's compact output with numbers spelled the way JavaScript prints a
//! double, so `1e21` becomes `1e+21` and an integer past 2^53 is the double it rounds to.

use std::io::{self, Write};

use crate::{constants::JS_MAX_EXACT_INTEGER, settings_json::js_float};

/// `JSON.stringify(value)`.
pub(crate) fn to_string<T: serde::Serialize + ?Sized>(value: &T) -> Result<String, serde_json::Error> {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsFormatter);
    value.serialize(&mut serializer)?;
    String::from_utf8(out).map_err(|error| serde::ser::Error::custom(error.to_string()))
}

struct JsFormatter;

impl serde_json::ser::Formatter for JsFormatter {
    fn write_i64<W: ?Sized + Write>(&mut self, writer: &mut W, value: i64) -> io::Result<()> {
        if value.unsigned_abs() <= JS_MAX_EXACT_INTEGER {
            write!(writer, "{value}")
        } else {
            #[allow(clippy::cast_precision_loss, reason = "JavaScript holds every number as a double")]
            let value = value as f64;
            self.write_f64(writer, value)
        }
    }

    fn write_u64<W: ?Sized + Write>(&mut self, writer: &mut W, value: u64) -> io::Result<()> {
        if value <= JS_MAX_EXACT_INTEGER {
            write!(writer, "{value}")
        } else {
            #[allow(clippy::cast_precision_loss, reason = "JavaScript holds every number as a double")]
            let value = value as f64;
            self.write_f64(writer, value)
        }
    }

    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_float(value).as_bytes())
    }
}

#[cfg(test)]
#[path = "js_json_tests.rs"]
mod tests;
