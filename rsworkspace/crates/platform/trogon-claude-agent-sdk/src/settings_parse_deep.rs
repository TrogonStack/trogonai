//! `JSON.parse` for settings text nested deeper than `serde_json` reads. V8 parses JSON without recursion, so a
//! settings file nests as deep as the reader can then walk it, where `serde_json` stops at
//! [`SERDE_JSON_NESTING`] levels. Here arrays and objects are built on an explicit stack, and each string, number
//! and literal is read by [`parse_json`], so it reads as it does in a shallow document.

use std::mem;

use serde_json::{Map, Value};

use crate::{constants::SERDE_JSON_NESTING, settings_parse::parse_json};

/// Whether `text` opens more arrays and objects at once, outside strings, than `serde_json` reads.
pub(crate) fn nests_past_serde_json(text: &str) -> bool {
    nests_past(text, SERDE_JSON_NESTING)
}

/// Whether `text` opens more than `limit` arrays and objects at once, outside strings. It stops counting at the
/// first level past `limit`, and says nothing about whether `text` is JSON.
pub(crate) fn nests_past(text: &str, limit: usize) -> bool {
    let bytes = text.as_bytes();
    let mut depth = 0_usize;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => {
                index = string_end(bytes, index.saturating_add(1));
                continue;
            }
            b'[' | b'{' => {
                depth = depth.saturating_add(1);
                if depth > limit {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        index = index.saturating_add(1);
    }
    false
}

/// An array or object whose closing bracket the parse has not reached, with the key the next member goes under.
enum Open {
    Array(Vec<Value>),
    Object(Map<String, Value>, String),
}

/// `JSON.parse(text)`, as [`parse_json`] reads it at any depth. `None` for text `JSON.parse` rejects, or that nests
/// more than `max_depth` arrays and objects, which no settings file the reader reads holds.
pub(crate) fn parse_json_deep(text: &str, max_depth: usize) -> Option<Value> {
    let mut reader = Reader { text, at: 0 };
    let mut open: Vec<Open> = Vec::new();
    loop {
        reader.skip_space();
        let mut value = match reader.peek()? {
            opening @ (b'[' | b'{') => {
                if open.len() >= max_depth {
                    return None;
                }
                reader.advance();
                reader.skip_space();
                match (opening, reader.peek()?) {
                    (b'[', b']') => {
                        reader.advance();
                        Value::Array(Vec::new())
                    }
                    (b'[', _) => {
                        open.push(Open::Array(Vec::new()));
                        continue;
                    }
                    (_, b'}') => {
                        reader.advance();
                        Value::Object(Map::new())
                    }
                    _ => {
                        let key = reader.key()?;
                        open.push(Open::Object(Map::new(), key));
                        continue;
                    }
                }
            }
            _ => reader.scalar()?,
        };
        loop {
            reader.skip_space();
            let Some(top) = open.last_mut() else {
                return (reader.at == text.len()).then_some(value);
            };
            let closed = match top {
                Open::Array(items) => {
                    items.push(value);
                    match reader.take_byte()? {
                        b',' => None,
                        b']' => open.pop(),
                        _ => return None,
                    }
                }
                Open::Object(entries, key) => {
                    entries.insert(mem::take(key), value);
                    match reader.take_byte()? {
                        b',' => {
                            reader.skip_space();
                            *key = reader.key()?;
                            None
                        }
                        b'}' => open.pop(),
                        _ => return None,
                    }
                }
            };
            value = match closed {
                None => break,
                Some(Open::Array(items)) => Value::Array(items),
                Some(Open::Object(entries, _)) => Value::Object(entries),
            };
        }
    }
}

/// Whether `JSON.parse` reads `text` as an object, checked at any depth without building the value: `None` for
/// text it rejects.
pub(crate) fn parses_as_object(text: &str) -> Option<bool> {
    let mut reader = Reader { text, at: 0 };
    let mut closers: Vec<u8> = Vec::new();
    reader.skip_space();
    let object = reader.peek()? == b'{';
    loop {
        reader.skip_space();
        match reader.peek()? {
            opening @ (b'[' | b'{') => {
                reader.advance();
                reader.skip_space();
                match (opening, reader.peek()?) {
                    (b'[', b']') | (b'{', b'}') => reader.advance(),
                    (b'[', _) => {
                        closers.push(b']');
                        continue;
                    }
                    _ => {
                        reader.key()?;
                        closers.push(b'}');
                        continue;
                    }
                }
            }
            _ => {
                reader.scalar()?;
            }
        }
        loop {
            reader.skip_space();
            let Some(&closer) = closers.last() else {
                return (reader.at == text.len()).then_some(object);
            };
            match reader.take_byte()? {
                b',' if closer == b'}' => {
                    reader.skip_space();
                    reader.key()?;
                    break;
                }
                b',' => break,
                byte if byte == closer => {
                    closers.pop();
                }
                _ => return None,
            }
        }
    }
}

struct Reader<'a> {
    text: &'a str,
    at: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn advance(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    fn take_byte(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.advance();
        Some(byte)
    }

    /// Skips the whitespace JSON allows between tokens.
    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.advance();
        }
    }

    /// Reads a member name and the colon after it.
    fn key(&mut self) -> Option<String> {
        if self.peek()? != b'"' {
            return None;
        }
        let key = self.scalar()?.as_str()?.to_owned();
        self.skip_space();
        (self.take_byte()? == b':').then_some(key)
    }

    /// Reads a string, number or literal.
    fn scalar(&mut self) -> Option<Value> {
        let bytes = self.text.as_bytes();
        let start = self.at;
        self.at = match self.peek()? {
            b'"' => string_end(bytes, start.saturating_add(1)),
            b'-' | b'0'..=b'9' => run_end(bytes, start, |byte| {
                matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E')
            }),
            byte if byte.is_ascii_alphabetic() => run_end(bytes, start, |byte| byte.is_ascii_alphabetic()),
            _ => return None,
        };
        parse_json(self.text.get(start..self.at)?).ok()
    }
}

fn run_end(bytes: &[u8], mut index: usize, accepts: impl Fn(u8) -> bool) -> usize {
    while bytes.get(index).is_some_and(|&byte| accepts(byte)) {
        index = index.saturating_add(1);
    }
    index
}

/// The index just past the closing quote of the string whose contents start at `index`, or the end of the text.
fn string_end(bytes: &[u8], mut index: usize) -> usize {
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' => index = index.saturating_add(2),
            b'"' => return index.saturating_add(1),
            _ => index = index.saturating_add(1),
        }
    }
    bytes.len()
}

#[cfg(test)]
#[path = "settings_parse_deep_tests.rs"]
mod tests;
