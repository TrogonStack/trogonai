//! A JSON value read the way Python's `json.loads` reads it and written back the way `json.dumps` writes it.
//!
//! `serde_json` without `arbitrary_precision` turns an integer outside `i64`/`u64` into a lossy `f64`, and it
//! refuses `NaN`, `Infinity` and floats that overflow, all of which Python reads. The Python dialect of the
//! embedded MCP server echoes a JSON-RPC id back, so the id is kept as the exact text the peer wrote.

use crate::{constants, python_json::float_repr};

/// The text of one JSON value as `json.loads` read it, with the text `json.dumps` writes for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawJsonId {
    kind: Kind,
    dump: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Kind {
    /// A Python `int`, as its canonical decimal text (`-0` is `0`).
    Integer(String),
    String,
    Other,
}

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum RawJsonIdError {
    #[error("the text is not a JSON value that Python's json.loads reads")]
    Invalid,
    #[error(
        "an integer of {digits} digits exceeds the {} digit limit of Python",
        constants::PYTHON_INTEGER_DIGIT_LIMIT
    )]
    IntegerTooLong { digits: usize },
}

impl RawJsonId {
    /// Reads one JSON value, allowing the `NaN`, `Infinity` and `-Infinity` constants and floats that overflow.
    pub fn parse(text: &str) -> Result<Self, RawJsonIdError> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            text,
            position: 0,
        };
        parser.skip_whitespace();
        let node = parser.value(0)?;
        parser.skip_whitespace();
        if parser.position != parser.bytes.len() {
            return Err(RawJsonIdError::Invalid);
        }
        let kind = match &node {
            Node::Integer(digits) => Kind::Integer(digits.clone()),
            Node::String(_) => Kind::String,
            _ => Kind::Other,
        };
        let mut dump = String::new();
        node.write_to(&mut dump);
        Ok(Self { kind, dump })
    }

    /// What `json.dumps(json.loads(text))` prints.
    pub fn python_dump(&self) -> &str {
        &self.dump
    }

    /// The canonical decimal digits, with a leading `-` for a negative number, when the value is a Python `int`.
    pub fn integer(&self) -> Option<&str> {
        match &self.kind {
            Kind::Integer(digits) => Some(digits),
            _ => None,
        }
    }

    pub fn is_string(&self) -> bool {
        self.kind == Kind::String
    }
}

enum Node {
    Integer(String),
    Float(f64),
    String(Vec<u16>),
    Bool(bool),
    Null,
    Array(Vec<Node>),
    /// Python's `dict`: a repeated key keeps its first position and takes its last value.
    Object(Vec<(Vec<u16>, Node)>),
}

impl Node {
    fn write_to(&self, out: &mut String) {
        match self {
            Self::Integer(digits) => out.push_str(digits),
            Self::Float(value) => out.push_str(&float_repr(*value)),
            Self::String(units) => write_string(units, out),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Null => out.push_str("null"),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    item.write_to(out);
                }
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    write_string(key, out);
                    out.push_str(": ");
                    value.write_to(out);
                }
                out.push('}');
            }
        }
    }
}

/// `json.dumps` with `ensure_ascii`: everything outside printable ASCII is a `\uXXXX` escape per UTF-16 unit.
fn write_string(units: &[u16], out: &mut String) {
    out.push('"');
    for &unit in units {
        match unit {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            0x20..=0x7e => {
                if let Some(ch) = char::from_u32(u32::from(unit)) {
                    out.push(ch);
                }
            }
            _ => {
                out.push_str("\\u");
                for shift in [12_u32, 8, 4, 0] {
                    let nibble = u32::from((unit >> shift) & 0xf);
                    out.extend(char::from_digit(nibble, 16));
                }
            }
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn eat(&mut self, literal: &str) -> bool {
        if self.bytes[self.position..].starts_with(literal.as_bytes()) {
            self.position += literal.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node, RawJsonIdError> {
        if depth > constants::RAW_JSON_MAX_DEPTH {
            return Err(RawJsonIdError::Invalid);
        }
        match self.peek().ok_or(RawJsonIdError::Invalid)? {
            b'"' => self.string().map(Node::String),
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'n' if self.eat("null") => Ok(Node::Null),
            b't' if self.eat("true") => Ok(Node::Bool(true)),
            b'f' if self.eat("false") => Ok(Node::Bool(false)),
            b'N' if self.eat("NaN") => Ok(Node::Float(f64::NAN)),
            b'I' if self.eat("Infinity") => Ok(Node::Float(f64::INFINITY)),
            b'-' if self.eat("-Infinity") => Ok(Node::Float(f64::NEG_INFINITY)),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(RawJsonIdError::Invalid),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.position;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
        self.position - start
    }

    /// Python's `NUMBER_RE`: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`, taking the longest match.
    fn number(&mut self) -> Result<Node, RawJsonIdError> {
        let start = self.position;
        let negative = self.peek() == Some(b'-');
        if negative {
            self.position += 1;
        }
        let digits_start = self.position;
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(RawJsonIdError::Invalid),
        }
        let integer_end = self.position;
        let mut float = false;
        if self.peek() == Some(b'.') && matches!(self.bytes.get(self.position + 1), Some(b'0'..=b'9')) {
            self.position += 1;
            self.digits();
            float = true;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mut lookahead = self.position + 1;
            if matches!(self.bytes.get(lookahead), Some(b'+' | b'-')) {
                lookahead += 1;
            }
            if matches!(self.bytes.get(lookahead), Some(b'0'..=b'9')) {
                self.position = lookahead;
                self.digits();
                float = true;
            }
        }
        if float {
            let literal = &self.text[start..self.position];
            return literal
                .parse::<f64>()
                .map(Node::Float)
                .map_err(|_| RawJsonIdError::Invalid);
        }
        let digits = &self.text[digits_start..integer_end];
        if digits.len() > constants::PYTHON_INTEGER_DIGIT_LIMIT {
            return Err(RawJsonIdError::IntegerTooLong { digits: digits.len() });
        }
        let canonical = if negative && digits != "0" {
            format!("-{digits}")
        } else {
            digits.to_owned()
        };
        Ok(Node::Integer(canonical))
    }

    fn hex4(&mut self) -> Result<u16, RawJsonIdError> {
        let digits = self
            .text
            .get(self.position..self.position + 4)
            .ok_or(RawJsonIdError::Invalid)?;
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(RawJsonIdError::Invalid);
        }
        self.position += 4;
        u16::from_str_radix(digits, 16).map_err(|_| RawJsonIdError::Invalid)
    }

    /// A string as UTF-16 units, so that an escaped lone surrogate survives as Python keeps it.
    fn string(&mut self) -> Result<Vec<u16>, RawJsonIdError> {
        self.position += 1;
        let mut units = Vec::new();
        loop {
            let start = self.position;
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.position += 1;
            }
            units.extend(self.text[start..self.position].encode_utf16());
            match self.peek().ok_or(RawJsonIdError::Invalid)? {
                b'"' => {
                    self.position += 1;
                    return Ok(units);
                }
                b'\\' => {
                    self.position += 1;
                    let escape = self.peek().ok_or(RawJsonIdError::Invalid)?;
                    self.position += 1;
                    match escape {
                        b'"' => units.push(0x22),
                        b'\\' => units.push(0x5c),
                        b'/' => units.push(0x2f),
                        b'b' => units.push(0x08),
                        b'f' => units.push(0x0c),
                        b'n' => units.push(0x0a),
                        b'r' => units.push(0x0d),
                        b't' => units.push(0x09),
                        b'u' => units.push(self.hex4()?),
                        _ => return Err(RawJsonIdError::Invalid),
                    }
                }
                _ => return Err(RawJsonIdError::Invalid),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Node, RawJsonIdError> {
        self.position += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Node::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return Ok(Node::Array(items));
                }
                _ => return Err(RawJsonIdError::Invalid),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Node, RawJsonIdError> {
        self.position += 1;
        let mut members: Vec<(Vec<u16>, Node)> = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Node::Object(members));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(RawJsonIdError::Invalid);
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(RawJsonIdError::Invalid);
            }
            self.position += 1;
            self.skip_whitespace();
            let value = self.value(depth + 1)?;
            match members.iter_mut().find(|(existing, _)| *existing == key) {
                Some(slot) => slot.1 = value,
                None => members.push((key, value)),
            }
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(Node::Object(members));
                }
                _ => return Err(RawJsonIdError::Invalid),
            }
        }
    }
}

#[cfg(test)]
#[path = "raw_json_id_tests.rs"]
mod tests;
