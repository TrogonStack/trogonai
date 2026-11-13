//! `JSON.parse` for settings text. A number token past the double range reads as `Infinity` upstream, where
//! `serde_json` refuses the whole document, so such a token is handed to `serde_json` as `0` and the value read
//! for it becomes the infinity of its sign. A finite token whose double is an infinity marker is read as the equal
//! integer, so it keeps its value.

use std::cell::Cell;

use serde::de::{DeserializeSeed, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::settings_numbers::{finite_number, infinity, is_marker_value};

/// `JSON.parse(text)`, reading a number past the double range as an infinity.
pub(crate) fn parse_json(text: &str) -> Result<Value, serde_json::Error> {
    let (spelled, tokens, holds_marker_value) = number_tokens(text);
    if !holds_marker_value && tokens.iter().all(Option::is_none) {
        return serde_json::from_str(text);
    }
    let next = Cell::new(0);
    let mut deserializer = serde_json::Deserializer::from_str(&spelled);
    let value = Seeded {
        tokens: &tokens,
        next: &next,
    }
    .deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

/// Every number token of `text` outside strings, in order, as `Some(negative)` for a token past the double range
/// and `None` otherwise, plus `text` with each such token spelled `0`, and whether a finite token reads as the
/// double of an infinity marker.
fn number_tokens(text: &str) -> (String, Vec<Option<bool>>, bool) {
    let bytes = text.as_bytes();
    let mut spelled = String::with_capacity(text.len());
    let mut tokens = Vec::new();
    let mut copied = 0;
    let mut index = 0;
    let mut holds_marker_value = false;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => index = string_end(bytes, index + 1),
            b'-' | b'0'..=b'9' => {
                let start = index;
                while bytes
                    .get(index)
                    .is_some_and(|byte| matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E'))
                {
                    index += 1;
                }
                let value = text
                    .get(start..index)
                    .filter(|token| is_json_number(token))
                    .and_then(|token| token.parse::<f64>().ok());
                holds_marker_value |= value.is_some_and(is_marker_value);
                let overflow = value.filter(|value| value.is_infinite()).map(f64::is_sign_negative);
                if overflow.is_some() {
                    spelled.push_str(text.get(copied..start).unwrap_or_default());
                    spelled.push('0');
                    copied = index;
                }
                tokens.push(overflow);
            }
            _ => index += 1,
        }
    }
    spelled.push_str(text.get(copied..).unwrap_or_default());
    (spelled, tokens, holds_marker_value)
}

/// Whether `token` is a number in the JSON grammar, so that spelling it `0` keeps invalid text invalid.
fn is_json_number(token: &str) -> bool {
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

/// Builds the `Value` `serde_json` would, except that the number read for an overflowing token is its infinity.
/// `serde_json` visits values in text order, so the n-th number visited is the n-th number token.
#[derive(Clone, Copy)]
struct Seeded<'a> {
    tokens: &'a [Option<bool>],
    next: &'a Cell<usize>,
}

impl Seeded<'_> {
    fn number(self, read: Value) -> Value {
        let index = self.next.get();
        self.next.set(index + 1);
        match self.tokens.get(index).copied().flatten() {
            Some(negative) => Value::Number(infinity(negative)),
            None => read,
        }
    }
}

impl<'de> DeserializeSeed<'de> for Seeded<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seeded<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        Ok(self.number(Value::Number(value.into())))
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        Ok(self.number(Value::Number(value.into())))
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        Ok(self.number(finite_number(value)))
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = access.next_element_seed(self)? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut entries = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            let value = access.next_value_seed(self)?;
            entries.insert(key, value);
        }
        Ok(Value::Object(entries))
    }
}

#[cfg(test)]
#[path = "settings_parse_tests.rs"]
mod tests;
