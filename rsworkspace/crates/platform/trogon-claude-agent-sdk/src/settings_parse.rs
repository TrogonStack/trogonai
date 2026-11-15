//! `JSON.parse` for settings text. A number token past the double range reads as `Infinity` upstream, where
//! `serde_json` refuses the whole document, so such a token is handed to `serde_json` as `0` and the value read
//! for it becomes the infinity of its sign. A finite token whose double is an infinity marker is read as the equal
//! integer, so it keeps its value.

use std::cell::Cell;

use serde::de::{DeserializeSeed, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::{
    json_scan::{Token, overflows, scan},
    settings_numbers::{finite_number, infinity, is_marker_value},
};

/// `JSON.parse(text)`, reading a number past the double range as an infinity.
pub(crate) fn parse_json(text: &str) -> Result<Value, serde_json::Error> {
    let scanned = scan(text, false);
    if !scanned.respelled && !scanned.tokens.iter().any(holds_marker_value) {
        return serde_json::from_str(text);
    }
    let next = Cell::new(0);
    let mut deserializer = serde_json::Deserializer::from_str(&scanned.text);
    let value = Seeded {
        tokens: &scanned.tokens,
        next: &next,
    }
    .deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

/// Whether `token` is a finite number whose double is an infinity marker.
fn holds_marker_value(token: &Token<'_>) -> bool {
    match token {
        Token::Number(spelling) => spelling.parse::<f64>().is_ok_and(is_marker_value),
        _ => false,
    }
}

/// Builds the `Value` `serde_json` would, except that the number read for an overflowing token is its infinity.
/// `serde_json` visits values in text order, so the n-th number or `null` visited is the n-th token of the scan.
#[derive(Clone, Copy)]
struct Seeded<'a> {
    tokens: &'a [Token<'a>],
    next: &'a Cell<usize>,
}

impl Seeded<'_> {
    /// Takes the next token: `Some(negative)` when it is a number past the double range.
    fn advance(self) -> Option<bool> {
        let index = self.next.get();
        self.next.set(index + 1);
        match self.tokens.get(index) {
            Some(Token::Number(spelling)) => overflows(spelling),
            _ => None,
        }
    }

    fn number(self, read: Value) -> Value {
        self.advance()
            .map_or(read, |negative| Value::Number(infinity(negative)))
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

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        self.advance();
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
