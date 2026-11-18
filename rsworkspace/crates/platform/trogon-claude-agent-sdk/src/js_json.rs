//! TypeScript `JSON.stringify`: serde_json's compact output with numbers spelled the way JavaScript prints a
//! double, so `1e21` becomes `1e+21` and an integer past 2^53 is the double it rounds to, and every object's keys
//! in JavaScript enumeration order, array indices ascending first.

use std::io::{self, Write};

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::{constants::JS_MAX_EXACT_INTEGER, js_object_order::js_key_order, settings_json::js_float};

/// `JSON.stringify(value)`. A JavaScript object enumerates its array-index keys first whatever order they were
/// set in, so a key such as `"0"` in a tool input, hook output or MCP result is written ahead of the named keys.
pub(crate) fn to_string(value: &Value) -> Result<String, serde_json::Error> {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, JsFormatter);
    JsOrdered(value).serialize(&mut serializer)?;
    String::from_utf8(out).map_err(|error| serde::ser::Error::custom(error.to_string()))
}

/// A value serialized with every object's keys in JavaScript enumeration order, without copying it.
struct JsOrdered<'a>(&'a Value);

impl Serialize for JsOrdered<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Array(items) => serializer.collect_seq(items.iter().map(JsOrdered)),
            Value::Object(map) => {
                serializer.collect_map(js_key_order(map).into_iter().map(|(key, item)| (key, JsOrdered(item))))
            }
            other => other.serialize(serializer),
        }
    }
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

    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_float(value).as_bytes())
    }
}

#[cfg(test)]
#[path = "js_json_tests.rs"]
mod tests;
