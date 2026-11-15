//! The JSON-RPC ids of an `mcp_message` control request, kept as the text the CLI wrote.
//!
//! The Python SDK reads the line with `json.loads`, so an id is a Python `int` of any size, a `float` (including
//! `Infinity`), a `bool`, `None`, a string or a nested value, and `json.dumps` writes it back in its own spelling.
//! `serde_json` here has no `arbitrary_precision`, so those ids cannot live in a `Value`. A transport scans the
//! line for the two ids the Python dialect reads (`message.id` and `message.params.requestId`), parses the line
//! with those ids replaced by `null`, and hands the exact ids to the driver, which puts a marker string where the
//! id is echoed and swaps the marker for the Python spelling when it writes the line.

use std::{hash::BuildHasher, sync::OnceLock};

use serde_json::Value;

use crate::{JsonObject, constants, raw_json_id::RawJsonId};

/// The ids of one `mcp_message` control request that a `Value` cannot hold exactly.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpWireIds {
    id: Option<RawJsonId>,
    requested: Option<RawJsonId>,
}

/// A line scanned for the ids of an `mcp_message` control request.
pub struct McpWireScan {
    ids: McpWireIds,
    parseable: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

impl McpWireScan {
    /// Python reads the id as a `json.loads` value; the scan tolerates the constants and overflowing floats that
    /// `serde_json` refuses.
    pub fn new(frame: &[u8]) -> Self {
        let empty = Self {
            ids: McpWireIds::default(),
            parseable: None,
        };
        if !frame
            .windows(b"mcp_message".len())
            .any(|window| window == b"mcp_message")
        {
            return empty;
        }
        let Some((ids, requests)) = locate(frame) else {
            return empty;
        };
        let read = |span: Option<Span>| {
            span.and_then(|span| {
                let text = std::str::from_utf8(frame.get(span.start..span.end)?).ok()?;
                RawJsonId::parse(text)
                    .ok()
                    .filter(|raw| !raw.is_string())
                    .map(|raw| (span, raw))
            })
        };
        let id = read(ids.last().copied());
        let requested = read(requests.last().copied());
        // `json.loads` drops an earlier value of a repeated key whatever it holds, even a constant `serde_json`
        // refuses, so every earlier id is replaced too.
        let mut spans: Vec<Span> = id
            .iter()
            .chain(requested.iter())
            .map(|(span, _)| *span)
            .chain(shadowed(&ids).iter().copied())
            .chain(shadowed(&requests).iter().copied())
            .collect();
        spans.sort_by_key(|span| std::cmp::Reverse(span.start));
        let parseable = (!spans.is_empty()).then(|| {
            let mut bytes = frame.to_vec();
            for span in spans {
                bytes.splice(span.start..span.end, *b"null").for_each(drop);
            }
            bytes
        });
        Self {
            ids: McpWireIds {
                id: id.map(|(_, raw)| raw),
                requested: requested.map(|(_, raw)| raw),
            },
            parseable,
        }
    }

    /// The bytes to hand to `serde_json`: the line itself unless an id had to be replaced by `null`.
    pub fn frame<'a>(&'a self, original: &'a [u8]) -> &'a [u8] {
        self.parseable.as_deref().unwrap_or(original)
    }

    /// `None` when the line carries no id that a `Value` would not keep.
    pub fn into_ids(self) -> Option<McpWireIds> {
        (self.ids != McpWireIds::default()).then_some(self.ids)
    }
}

impl McpWireIds {
    /// Puts each id back into the message the way the dialect must read it: an integer as itself, or as a marker
    /// string when it does not fit 64 bits. Any other kind stays `null`, which the Python server reads as no id.
    pub(crate) fn apply_to(&self, message: &mut JsonObject) {
        if let Some(value) = self.id.as_ref().and_then(dialect_value) {
            message.insert("id".to_owned(), value);
        }
        if let Some(value) = self.requested.as_ref().and_then(dialect_value)
            && let Some(Value::Object(params)) = message.get_mut("params")
        {
            params.insert("requestId".to_owned(), value);
        }
    }

    /// Marks the id of a response, when it has one, to be written in Python's spelling of the original id.
    pub(crate) fn echo(&self, response: &mut Value) {
        if let Some(raw) = &self.id
            && let Value::Object(object) = response
            && object.contains_key("id")
        {
            object.insert("id".to_owned(), Value::String(marker(raw)));
        }
    }
}

fn dialect_value(raw: &RawJsonId) -> Option<Value> {
    let digits = raw.integer()?;
    if let Ok(integer) = digits.parse::<i64>() {
        return Some(Value::from(integer));
    }
    if let Ok(integer) = digits.parse::<u64>() {
        return Some(Value::from(integer));
    }
    Some(Value::String(marker(raw)))
}

/// A string that no peer can predict, carrying the Python spelling of the id it stands for.
fn marker(raw: &RawJsonId) -> String {
    format!(
        "{}{}:{}",
        constants::PYTHON_ID_MARKER_PREFIX,
        nonce(),
        hex(raw.python_dump())
    )
}

fn nonce() -> &'static str {
    static NONCE: OnceLock<String> = OnceLock::new();
    NONCE.get_or_init(|| {
        let first = std::collections::hash_map::RandomState::new().hash_one(0_u8);
        let second = std::collections::hash_map::RandomState::new().hash_one(1_u8);
        format!("{first:016x}{second:016x}")
    })
}

/// The Python spelling an integer marker stands for, which is also its `repr`.
pub(crate) fn marker_text(value: &Value) -> Option<String> {
    let Value::String(text) = value else {
        return None;
    };
    let rest = text
        .strip_prefix(constants::PYTHON_ID_MARKER_PREFIX)?
        .strip_prefix(nonce())?
        .strip_prefix(':')?;
    unhex(rest)
}

/// Swaps every marker in a line that `python_json::to_string` wrote for the id text it stands for.
pub(crate) fn splice_markers(text: String) -> String {
    let needle = format!("\"\\u0000pyid:{}:", nonce());
    if !text.contains(&needle) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find(&needle) {
        let after = &rest[start + needle.len()..];
        let Some(end) = after.find('"') else {
            break;
        };
        match unhex(&after[..end]) {
            Some(spelling) => {
                out.push_str(&rest[..start]);
                out.push_str(&spelling);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[..start + needle.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn hex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        out.extend(char::from_digit(u32::from(byte >> 4), 16));
        out.extend(char::from_digit(u32::from(byte & 0xf), 16));
    }
    out
}

fn unhex(text: &str) -> Option<String> {
    if text.len() & 1 == 1 {
        return None;
    }
    let bytes = text
        .as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

/// Every span of `request.message.id` and of `request.message.params.requestId`, in line order, reading the line
/// the way `json.loads` does: a repeated key takes its last value, so the last span of each is the one read.
fn locate(frame: &[u8]) -> Option<(Vec<Span>, Vec<Span>)> {
    let whole = Span {
        start: 0,
        end: frame.len(),
    };
    let top = members(frame, whole)?;
    if string_at(frame, last(&top, "type")?)? != "control_request" {
        return None;
    }
    let request = members(frame, last(&top, "request")?)?;
    if string_at(frame, last(&request, "subtype")?)? != "mcp_message" {
        return None;
    }
    let message = members(frame, last(&request, "message")?)?;
    let ids = every(&message, "id");
    let requested = last(&message, "params")
        .and_then(|params| members(frame, params))
        .map(|params| every(&params, "requestId"))
        .unwrap_or_default();
    Some((ids, requested))
}

fn every(members: &[(String, Span)], name: &str) -> Vec<Span> {
    members
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, span)| *span)
        .collect()
}

/// The spans a later one of the same key overrides.
fn shadowed(spans: &[Span]) -> &[Span] {
    match spans.split_last() {
        Some((_, earlier)) => earlier,
        None => &[],
    }
}

fn last(members: &[(String, Span)], name: &str) -> Option<Span> {
    members.iter().rev().find(|(key, _)| key == name).map(|(_, span)| *span)
}

fn string_at(frame: &[u8], span: Span) -> Option<String> {
    serde_json::from_slice::<String>(frame.get(span.start..span.end)?).ok()
}

fn skip_whitespace(frame: &[u8], mut position: usize) -> usize {
    while matches!(frame.get(position), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        position += 1;
    }
    position
}

/// The members of the object that `span` holds, each with the span of its value.
fn members(frame: &[u8], span: Span) -> Option<Vec<(String, Span)>> {
    let mut position = skip_whitespace(frame, span.start);
    if frame.get(position) != Some(&b'{') {
        return None;
    }
    position = skip_whitespace(frame, position + 1);
    let mut found = Vec::new();
    if frame.get(position) == Some(&b'}') {
        return Some(found);
    }
    loop {
        position = skip_whitespace(frame, position);
        let key_end = skip_string(frame, position)?;
        let key = string_at(
            frame,
            Span {
                start: position,
                end: key_end,
            },
        )
        .unwrap_or_default();
        position = skip_whitespace(frame, key_end);
        if frame.get(position) != Some(&b':') {
            return None;
        }
        position = skip_whitespace(frame, position + 1);
        let end = skip_value(frame, position)?;
        found.push((key, Span { start: position, end }));
        position = skip_whitespace(frame, end);
        match frame.get(position)? {
            b',' => position += 1,
            b'}' => return Some(found),
            _ => return None,
        }
    }
}

/// The end of the string that starts at `position`.
fn skip_string(frame: &[u8], position: usize) -> Option<usize> {
    if frame.get(position) != Some(&b'"') {
        return None;
    }
    let mut at = position + 1;
    loop {
        match frame.get(at)? {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
}

/// The end of the value that starts at `position`: a string, a balanced array or object, or a bare token.
fn skip_value(frame: &[u8], position: usize) -> Option<usize> {
    match frame.get(position)? {
        b'"' => skip_string(frame, position),
        b'{' | b'[' => {
            let mut depth = 0_usize;
            let mut at = position;
            loop {
                match frame.get(at)? {
                    b'"' => {
                        at = skip_string(frame, at)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at + 1);
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
        }
        _ => {
            let mut at = position;
            while let Some(byte) = frame.get(at) {
                if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                    break;
                }
                at += 1;
            }
            (at > position).then_some(at)
        }
    }
}

#[cfg(test)]
#[path = "mcp_wire_ids_tests.rs"]
mod tests;
