//! JSON values nested deeper than the native stack holds recursive walks over them: Python reads, compares and
//! drops values nested thousands of levels deep, so the SDK parses them on a thread with a large stack, and clones
//! and drops them with loops over explicit stacks instead of the recursive `serde_json` implementations.

use crate::JsonObject;
use crate::constants::{PARSE_STACK, PYTHON_INT_MAX_STR_DIGITS, PYTHON_READER_DEPTH, SHALLOW};
use serde::Deserialize;
use serde_json::Value;

/// The error `serde_json::from_slice` gives for `bytes` without the recursion limit, `None` when they read.
fn unbounded_error(bytes: &[u8]) -> Option<serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    deserializer.disable_recursion_limit();
    JsonObject::deserialize(&mut deserializer)
        .and_then(|_| deserializer.end())
        .err()
}

/// A JSON object read from `bytes` at any nesting, as `serde_json::from_slice` reads it but without recursion, so
/// a message nested past any native stack still reads, as `json.loads` and `JSON.parse` read it. A malformed
/// message fails with the error `serde_json` gives, found on a thread whose stack holds the recursive parser up to
/// where the text goes wrong.
pub(crate) fn parse_object(bytes: &[u8]) -> Result<JsonObject, serde_json::Error> {
    parse_object_on(&[PARSE_STACK], bytes)
}

/// [`parse_object`] with the error of a malformed message nested deeper than [`SHALLOW`] found on a thread whose
/// stack is the first of `stack_sizes` the system grants. Where no such thread can run, or the text goes wrong
/// deeper than a Python reader enters, the recursion limit of `serde_json` applies to the error.
fn parse_object_on(stack_sizes: &[usize], bytes: &[u8]) -> Result<JsonObject, serde_json::Error> {
    let depth = match parse_nested(bytes) {
        Ok(object) => return Ok(object),
        Err(depth) => depth,
    };
    let unbounded = || {
        let failed = (SHALLOW..=PYTHON_READER_DEPTH).contains(&depth);
        failed.then(|| crate::large_stack::on_large_stack(stack_sizes, || unbounded_error(bytes), || None, None))
    };
    let error = unbounded()
        .flatten()
        .or_else(|| serde_json::from_slice::<JsonObject>(bytes).err());
    Err(error.unwrap_or_else(|| serde::de::Error::custom("unparsed JSON")))
}

/// A container [`parse_nested`] is inside: the children it read and, for an object, the key of the next one.
enum Reading {
    Array(Vec<Value>),
    Object(JsonObject, String),
}

impl Reading {
    fn finish(self) -> Value {
        match self {
            Self::Array(items) => Value::Array(items),
            Self::Object(object, _) => Value::Object(object),
        }
    }
}

/// Where the JSON string whose opening quote is at `at` ends, past its closing quote.
fn quoted_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut next = at + 1;
    loop {
        match *bytes.get(next)? {
            b'"' => return Some(next + 1),
            b'\\' => next += 2,
            _ => next += 1,
        }
    }
}

/// The object key that starts at `at` and where the value after its `:` starts.
fn object_key(bytes: &[u8], at: usize) -> Option<(String, usize)> {
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    let end = quoted_end(bytes, at)?;
    let key = serde_json::from_slice(bytes.get(at..end)?).ok()?;
    let colon = skip_whitespace(bytes, end);
    (bytes.get(colon) == Some(&b':')).then(|| (key, skip_whitespace(bytes, colon + 1)))
}

/// The string, number or constant that starts at `at` and where it ends, read by `serde_json` itself.
fn parse_scalar(bytes: &[u8], at: usize) -> Option<(Value, usize)> {
    let end = if bytes.get(at) == Some(&b'"') {
        quoted_end(bytes, at)?
    } else {
        at + bytes
            .get(at..)?
            .iter()
            .take_while(|byte| !b" \t\n\r,:]}".contains(byte))
            .count()
    };
    Some((serde_json::from_slice(bytes.get(at..end)?).ok()?, end))
}

/// The JSON object `bytes` holds, read with an explicit stack of the containers it is inside. On failure, how
/// many containers were open where the text went wrong.
fn parse_nested(bytes: &[u8]) -> Result<JsonObject, usize> {
    match read_value(bytes)? {
        Value::Object(object) => Ok(object),
        other => {
            drop_value(other);
            Err(0)
        }
    }
}

/// The JSON value `bytes` holds, read as [`parse_nested`] reads it.
fn read_value(bytes: &[u8]) -> Result<Value, usize> {
    let mut open: Vec<Reading> = Vec::new();
    let read = read_nested(bytes, &mut open);
    open.into_iter().map(Reading::finish).for_each(drop_value);
    read
}

/// The JSON value `text` holds, as `serde_json::from_str` reads it but at any nesting and without recursion.
/// `None` for text `serde_json` rejects.
pub(crate) fn parse_value(text: &str) -> Option<Value> {
    read_value(text.as_bytes()).ok()
}

/// `serde_json::to_value(value)`, which fails on a value whose members under undeclared keys of a generated upstream
/// type nest past what `serde_json` reads back, falling back to reading the JSON text `value` writes at any nesting.
pub(crate) fn encode<T: serde::Serialize + ?Sized>(value: &T) -> Result<Value, serde_json::Error> {
    serde_json::to_value(value).or_else(|_| {
        let text = serde_json::to_string(value)?;
        parse_value(&text).ok_or_else(|| serde::de::Error::custom("the encoded JSON text does not parse"))
    })
}

fn read_nested(bytes: &[u8], open: &mut Vec<Reading>) -> Result<Value, usize> {
    let mut at = skip_whitespace(bytes, 0);
    loop {
        let failed = open.len();
        let mut value = match bytes.get(at) {
            Some(b'[') => {
                at = skip_whitespace(bytes, at + 1);
                if bytes.get(at) != Some(&b']') {
                    open.push(Reading::Array(Vec::new()));
                    continue;
                }
                at += 1;
                Value::Array(Vec::new())
            }
            Some(b'{') => {
                at = skip_whitespace(bytes, at + 1);
                if bytes.get(at) != Some(&b'}') {
                    let (key, next) = object_key(bytes, at).ok_or(failed)?;
                    open.push(Reading::Object(JsonObject::new(), key));
                    at = next;
                    continue;
                }
                at += 1;
                Value::Object(JsonObject::new())
            }
            _ => {
                let (scalar, end) = parse_scalar(bytes, at).ok_or(failed)?;
                at = end;
                scalar
            }
        };
        loop {
            let failed = open.len();
            let Some(innermost) = open.last_mut() else {
                if skip_whitespace(bytes, at) == bytes.len() {
                    return Ok(value);
                }
                drop_value(value);
                return Err(0);
            };
            at = skip_whitespace(bytes, at);
            let separator = bytes.get(at).copied();
            match innermost {
                Reading::Array(items) => {
                    items.push(value);
                    if separator == Some(b',') {
                        at = skip_whitespace(bytes, at + 1);
                        break;
                    }
                }
                Reading::Object(object, key) => {
                    if let Some(replaced) = object.insert(std::mem::take(key), value) {
                        drop_value(replaced);
                    }
                    if separator == Some(b',') {
                        let (next_key, next) = object_key(bytes, skip_whitespace(bytes, at + 1)).ok_or(failed)?;
                        *key = next_key;
                        at = next;
                        break;
                    }
                }
            }
            let closes = match innermost {
                Reading::Array(_) => separator == Some(b']'),
                Reading::Object(..) => separator == Some(b'}'),
            };
            if !closes {
                return Err(failed);
            }
            value = open.pop().map_or(Value::Null, Reading::finish);
            at += 1;
        }
    }
}

/// What `json.loads` raises on a CLI stdout line in the Python SDK's message reader other than
/// `JSONDecodeError`.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PythonRaise {
    /// `RecursionError`, with its text: the C scanner checks the recursion limit as it enters each object or array,
    /// so it raises at the first container past [`PYTHON_READER_DEPTH`].
    Recursion(&'static str),
    /// `ValueError` for an integer of this many digits, more than `int` converts.
    IntDigits(usize),
}

/// What `json.loads` raises on the stripped CLI stdout `line` other than `JSONDecodeError`, if it raises: the
/// first of a container past [`PYTHON_READER_DEPTH`] and an integer of more than [`PYTHON_INT_MAX_STR_DIGITS`] digits
/// that the scanner comes to without a `JSONDecodeError` first.
pub(crate) fn python_raise(line: &[u8]) -> Option<PythonRaise> {
    python_raise_past(line, PYTHON_READER_DEPTH)
}

/// A container CPython's JSON scanner is inside.
#[derive(Clone, Copy)]
enum Container {
    Array,
    Object,
}

/// [`python_raise`] for a reader that enters `limit` containers. It reads `text` strictly with the default hooks,
/// with the grammar of CPython 3.12's `scan_once_unicode`, `_parse_object_unicode` and `_parse_array_unicode`,
/// which also take `NaN`, `Infinity`, `-Infinity` and lone surrogate escapes. `None` where the scanner raises
/// `JSONDecodeError` first or reads the text without raising.
fn python_raise_past(text: &[u8], limit: usize) -> Option<PythonRaise> {
    let mut open = Vec::new();
    let mut next = skip_whitespace(text, 0);
    loop {
        let ended = match *text.get(next)? {
            bracket @ (b'[' | b'{') if open.len() >= limit => {
                return Some(PythonRaise::Recursion(if bracket == b'[' {
                    "maximum recursion depth exceeded while decoding a JSON array from a unicode string"
                } else {
                    "maximum recursion depth exceeded while decoding a JSON object from a unicode string"
                }));
            }
            b'[' => {
                next = skip_whitespace(text, next + 1);
                if text.get(next) != Some(&b']') {
                    open.push(Container::Array);
                    continue;
                }
                next + 1
            }
            b'{' => {
                next = skip_whitespace(text, next + 1);
                if text.get(next) != Some(&b'}') {
                    next = member_value(text, next)?;
                    open.push(Container::Object);
                    continue;
                }
                next + 1
            }
            b'"' => string_end(text, next + 1)?,
            _ => match scalar_end(text, next)? {
                Scalar::End(end) => end,
                Scalar::LongInteger(digits) => return Some(PythonRaise::IntDigits(digits)),
            },
        };
        next = after_value(text, ended, &mut open)?;
    }
}

/// Where the next value starts once a value ends at `ended`: the scanner closes the containers that end there and
/// reads past the `,` and, in an object, the next key. `None` where it raises, or where the outermost value ends.
fn after_value(text: &[u8], ended: usize, open: &mut Vec<Container>) -> Option<usize> {
    let mut next = ended;
    loop {
        let container = *open.last()?;
        next = skip_whitespace(text, next);
        match (container, text.get(next)) {
            (Container::Array, Some(b',')) => return Some(skip_whitespace(text, next + 1)),
            (Container::Object, Some(b',')) => return member_value(text, skip_whitespace(text, next + 1)),
            (Container::Array, Some(b']')) | (Container::Object, Some(b'}')) => {
                open.pop();
                next += 1;
            }
            _ => return None,
        }
    }
}

/// Where the value of the object member whose key starts at `at` starts, past the key, the `:` and the whitespace
/// around it.
fn member_value(text: &[u8], at: usize) -> Option<usize> {
    if text.get(at) != Some(&b'"') {
        return None;
    }
    let colon = skip_whitespace(text, string_end(text, at + 1)?);
    (text.get(colon) == Some(&b':')).then(|| skip_whitespace(text, colon + 1))
}

/// Past the `IS_WHITESPACE` characters of CPython's JSON scanner from `at`.
fn skip_whitespace(text: &[u8], at: usize) -> usize {
    at + text.get(at..).map_or(0, |rest| {
        rest.iter()
            .take_while(|&&byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
            .count()
    })
}

/// Past the closing quote of the string whose body starts at `at`, as `scanstring_unicode` reads it strictly: no
/// control characters, and only the escapes JSON defines, where any four hex digits make a `\u` escape.
fn string_end(text: &[u8], at: usize) -> Option<usize> {
    let mut next = at;
    loop {
        match *text.get(next)? {
            b'"' => return Some(next + 1),
            b'\\' => {
                next += match *text.get(next + 1)? {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => 2,
                    b'u' if text.get(next + 2..next + 6)?.iter().all(u8::is_ascii_hexdigit) => 6,
                    _ => return None,
                };
            }
            byte if byte < 0x20 => return None,
            _ => next += 1,
        }
    }
}

/// A constant or number the scanner matched.
enum Scalar {
    /// It ends here.
    End(usize),
    /// An integer of this many digits, more than `int` converts, where Python raises `ValueError`.
    LongInteger(usize),
}

/// The constant or number that starts at `at`, as `scan_once_unicode` and `_match_number_unicode` match it. `None`
/// where neither starts.
fn scalar_end(text: &[u8], at: usize) -> Option<Scalar> {
    let rest = text.get(at..)?;
    let constants: [&[u8]; 6] = [b"null", b"true", b"false", b"NaN", b"Infinity", b"-Infinity"];
    if let Some(constant) = constants.iter().find(|constant| rest.starts_with(constant)) {
        return Some(Scalar::End(at + constant.len()));
    }
    let (integer_end, end) = number_end(text, at)?;
    let digits = integer_end - at - usize::from(rest.first() == Some(&b'-'));
    Some(if integer_end == end && digits > PYTHON_INT_MAX_STR_DIGITS {
        Scalar::LongInteger(digits)
    } else {
        Scalar::End(end)
    })
}

/// Where the integer part of the JSON number that starts at `at` ends, and where the number ends, as
/// `_match_number_unicode` matches it: a fraction needs a digit after the `.` and an exponent one after its sign.
fn number_end(text: &[u8], at: usize) -> Option<(usize, usize)> {
    let digits_from = |from: usize| {
        from + text
            .get(from..)
            .map_or(0, |rest| rest.iter().take_while(|byte| byte.is_ascii_digit()).count())
    };
    let integer = at + usize::from(text.get(at) == Some(&b'-'));
    let integer_end = match text.get(integer)? {
        b'0' => integer + 1,
        b'1'..=b'9' => digits_from(integer),
        _ => return None,
    };
    let mut end = integer_end;
    if text.get(end) == Some(&b'.') && text.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end = digits_from(end + 1);
    }
    if matches!(text.get(end), Some(b'e' | b'E')) {
        let exponent = end + 1 + usize::from(matches!(text.get(end + 1), Some(b'-' | b'+')));
        let exponent_end = digits_from(exponent);
        if exponent_end > exponent {
            end = exponent_end;
        }
    }
    Some((integer_end, end))
}

/// `str.strip()` of the decoded line: the characters CPython's `Py_UNICODE_ISSPACE` takes, from both ends.
pub(crate) fn python_strip(line: &[u8]) -> &[u8] {
    let Ok(text) = std::str::from_utf8(line) else {
        return line;
    };
    text.trim_matches(|character: char| {
        matches!(
            character,
            '\t'..='\r'
                | '\u{1c}'..='\u{20}'
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
    })
    .as_bytes()
}

/// `text` with each integer `-0` outside strings spelled ` 0`: `json.loads` reads it as the integer `0`, where
/// `serde_json` reads the float `-0.0`. The spelling keeps every offset.
pub(crate) fn python_integer_zero(text: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let mut spelled = std::borrow::Cow::Borrowed(text);
    let mut next = 0;
    while let Some(&byte) = text.get(next) {
        next = match byte {
            b'"' => string_end(text, next + 1).unwrap_or(text.len()),
            b'-' if text.get(next + 1) == Some(&b'0') => match number_end(text, next) {
                Some((integer_end, end)) if integer_end == end => {
                    spelled.to_mut()[next] = b' ';
                    end
                }
                _ => next + 1,
            },
            _ => next + 1,
        };
    }
    spelled
}

/// The value `JSON.parse` returns for a CLI stdout line, as V8 holds it, at any nesting: an object lists its
/// array-index keys first, in ascending order, then its other keys in the order they came; and a number is the
/// nearest double, so an integer past 2^53 rounds. A rounded integer stays an integer while one fits.
pub(crate) fn javascript_object(object: &mut JsonObject) {
    let mut pending = Vec::new();
    javascript_members(object, &mut pending);
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(object) => javascript_members(object, &mut pending),
            Value::Array(items) => pending.extend(items.iter_mut()),
            Value::Number(number) => javascript_number(number),
            _ => {}
        }
    }
}

/// Puts the array-index keys of `object` first, as V8 orders them, and queues its values.
fn javascript_members<'a>(object: &'a mut JsonObject, pending: &mut Vec<&'a mut Value>) {
    if object.keys().any(|key| array_index(key).is_some()) {
        let (mut indexed, named): (Vec<_>, Vec<_>) = std::mem::take(object)
            .into_iter()
            .partition(|(key, _)| array_index(key).is_some());
        indexed.sort_by_key(|(key, _)| array_index(key));
        object.extend(indexed.into_iter().chain(named));
    }
    pending.extend(object.values_mut());
}

/// The array index `key` names: the canonical decimal of an integer below 2^32 - 1.
fn array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || !key.starts_with('0') && key.bytes().all(|byte| byte.is_ascii_digit());
    key.parse::<u32>().ok().filter(|index| canonical && *index < u32::MAX)
}

/// `number` as the double JavaScript reads from its text.
fn javascript_number(number: &mut serde_json::Number) {
    const EXACT: u64 = 1 << 53;
    let rounded = match (number.as_u64(), number.as_i64()) {
        (Some(integer), _) if integer > EXACT => integer as f64,
        (None, Some(integer)) if integer.unsigned_abs() > EXACT => integer as f64,
        _ => return,
    };
    if rounded < 0.0 {
        *number = serde_json::Number::from(rounded as i64);
    } else if rounded < u64::MAX as f64 {
        *number = serde_json::Number::from(rounded as u64);
    } else if let Some(float) = serde_json::Number::from_f64(rounded) {
        *number = float;
    }
}

/// Whether the CLI stdout `line` is the JSON `null`, which TypeScript's message loop reads `type` from.
pub(crate) fn javascript_null(line: &[u8]) -> bool {
    let blank = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\r');
    let start = line.iter().position(|byte| !blank(byte)).unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|byte| !blank(byte))
        .map_or(start, |last| last + 1);
    line.get(start..end) == Some(b"null".as_slice())
}

/// A container being cloned: the children left to clone, the ones already cloned and, for an object, the key of
/// the child being cloned.
enum Cloning<'a> {
    Array(std::slice::Iter<'a, Value>, Vec<Value>),
    Object(serde_json::map::Iter<'a>, JsonObject, String),
}

impl<'a> Cloning<'a> {
    fn start(value: &'a Value) -> Result<Self, Value> {
        match value {
            Value::Array(items) => Ok(Self::Array(items.iter(), Vec::with_capacity(items.len()))),
            Value::Object(object) => Ok(Self::Object(
                object.iter(),
                JsonObject::with_capacity(object.len()),
                String::new(),
            )),
            scalar => Err(scalar.clone()),
        }
    }

    fn store(&mut self, child: Value) {
        match self {
            Self::Array(_, items) => items.push(child),
            Self::Object(_, object, key) => {
                object.insert(std::mem::take(key), child);
            }
        }
    }

    fn next_child(&mut self) -> Option<&'a Value> {
        match self {
            Self::Array(children, _) => children.next(),
            Self::Object(children, _, key) => children.next().map(|(name, child)| {
                key.clone_from(name);
                child
            }),
        }
    }

    fn finish(self) -> Value {
        match self {
            Self::Array(_, items) => Value::Array(items),
            Self::Object(_, object, _) => Value::Object(object),
        }
    }
}

/// `value.clone()` at any nesting.
pub(crate) fn clone_value(value: &Value) -> Value {
    let mut open: Vec<Cloning<'_>> = Vec::new();
    let mut next = value;
    loop {
        let mut finished = match Cloning::start(next) {
            Ok(container) => {
                open.push(container);
                None
            }
            Err(scalar) => Some(scalar),
        };
        loop {
            let Some(innermost) = open.last_mut() else {
                return finished.unwrap_or_default();
            };
            if let Some(child) = finished.take() {
                innermost.store(child);
            }
            if let Some(child) = innermost.next_child() {
                next = child;
                break;
            }
            finished = open.pop().map(Cloning::finish);
        }
    }
}

/// `object.clone()` at any nesting.
pub(crate) fn clone_object(object: &JsonObject) -> JsonObject {
    object
        .iter()
        .map(|(key, value)| (key.clone(), clone_value(value)))
        .collect()
}

/// Drops `value` at any nesting, taking each container apart before it drops.
pub(crate) fn drop_value(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items),
            Value::Object(object) => pending.extend(object.into_iter().map(|(_, child)| child)),
            _ => {}
        }
    }
}

/// Drops the value `slot` holds at any nesting, leaving `null` in its place.
pub(crate) fn take_and_drop(slot: &mut Value) {
    drop_value(std::mem::take(slot));
}

/// Drops the values of `object` at any nesting, leaving it empty.
pub(crate) fn clear_object(object: &mut JsonObject) {
    std::mem::take(object)
        .into_iter()
        .for_each(|(_, value)| drop_value(value));
}

/// The JSON values a type owns, visited one by one so the crate can rebuild or take apart values nested past the
/// native stack without walking the type recursively.
pub(crate) trait OwnedValues {
    fn each_value(&mut self, visit: &mut dyn FnMut(&mut Value));

    /// Drops every value this owns at any nesting, leaving `null` in its place.
    fn drop_values(&mut self) {
        self.each_value(&mut take_and_drop);
    }
}

impl OwnedValues for Value {
    fn each_value(&mut self, visit: &mut dyn FnMut(&mut Value)) {
        visit(self);
    }
}

impl OwnedValues for JsonObject {
    fn each_value(&mut self, visit: &mut dyn FnMut(&mut Value)) {
        self.values_mut().for_each(visit);
    }
}

impl<T: OwnedValues> OwnedValues for Option<T> {
    fn each_value(&mut self, visit: &mut dyn FnMut(&mut Value)) {
        if let Some(owned) = self {
            owned.each_value(visit);
        }
    }
}

impl<T: OwnedValues> OwnedValues for Vec<T> {
    fn each_value(&mut self, visit: &mut dyn FnMut(&mut Value)) {
        self.iter_mut().for_each(|owned| owned.each_value(visit));
    }
}

/// `Drop` for types that own JSON values: each value is taken apart with an explicit stack, so a value nested past
/// the native stack drops without overflowing it.
macro_rules! drop_values_iteratively {
    ($($owner:ty),* $(,)?) => {
        $(impl Drop for $owner {
            fn drop(&mut self) {
                crate::deep_json::OwnedValues::drop_values(self);
            }
        })*
    };
}
pub(crate) use drop_values_iteratively;

/// A JSON object that drops at any nesting. It reads and writes as the [`JsonObject`] it derefs to, clones with a
/// loop, and takes its values apart with an explicit stack when it drops. `Message::Unknown` and `ContentBlock`
/// hold their JSON in it, so their variants stay open to destructuring, which a `Drop` on the enums themselves
/// would close.
#[derive(Default)]
pub struct DeepJsonObject(JsonObject);

impl DeepJsonObject {
    /// The object this holds.
    pub fn as_object(&self) -> &JsonObject {
        &self.0
    }

    /// The object this holds, taken out. It then drops as `serde_json` drops it.
    pub fn into_object(mut self) -> JsonObject {
        std::mem::take(&mut self.0)
    }
}

impl Clone for DeepJsonObject {
    fn clone(&self) -> Self {
        Self(clone_object(&self.0))
    }
}

impl From<DeepJsonObject> for JsonObject {
    fn from(object: DeepJsonObject) -> Self {
        object.into_object()
    }
}

/// A JSON value that drops at any nesting, as [`DeepJsonObject`] holds an object. It also compares and prints with
/// loops, as `serde_json::Value` compares and prints, and [`DeepJsonValue::to_json`] writes it at any nesting.
/// Each value under an undeclared key of a generated upstream type, such as `Settings`, is one.
#[derive(Default)]
pub struct DeepJsonValue(Value);

impl DeepJsonValue {
    /// The value this holds.
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// The value this holds, taken out. It then drops as `serde_json` drops it.
    pub fn into_value(mut self) -> Value {
        std::mem::take(&mut self.0)
    }

    /// `serde_json::to_string(self)` at any nesting, written with a loop.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        compact(&self.0)
    }

    /// `serde_json::to_value(value)` for a value holding generated upstream types, such as `Settings`, at any
    /// nesting. Such a type writes each member under an undeclared key nested past [`SHALLOW`] levels as its JSON
    /// text, which `serde_json::to_value` reads back only up to the 128 levels `serde_json` reads; this reads it
    /// back without recursion.
    pub fn encode<T: serde::Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        encode(value).map(Self)
    }
}

impl Clone for DeepJsonValue {
    fn clone(&self) -> Self {
        Self(clone_value(&self.0))
    }
}

impl From<DeepJsonValue> for Value {
    fn from(value: DeepJsonValue) -> Self {
        value.into_value()
    }
}

impl std::fmt::Debug for DeepJsonValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        debug_value(&self.0, formatter)
    }
}

impl PartialEq for DeepJsonValue {
    fn eq(&self, other: &Self) -> bool {
        equal(&self.0, &other.0)
    }
}

impl Eq for DeepJsonValue {}

impl PartialEq<Value> for DeepJsonValue {
    fn eq(&self, other: &Value) -> bool {
        equal(&self.0, other)
    }
}

impl std::fmt::Debug for DeepJsonObject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, formatter)
    }
}

impl PartialEq for DeepJsonObject {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for DeepJsonObject {}

impl PartialEq<JsonObject> for DeepJsonObject {
    fn eq(&self, other: &JsonObject) -> bool {
        self.0 == *other
    }
}

/// The traits [`DeepJsonObject`] and [`DeepJsonValue`] share with the JSON they hold, and the `Drop` that takes it
/// apart with an explicit stack.
macro_rules! deep_json_holder {
    ($($holder:ident($held:ty)),* $(,)?) => {
        $(
            impl std::ops::Deref for $holder {
                type Target = $held;

                fn deref(&self) -> &$held {
                    &self.0
                }
            }

            impl std::ops::DerefMut for $holder {
                fn deref_mut(&mut self) -> &mut $held {
                    &mut self.0
                }
            }

            impl From<$held> for $holder {
                fn from(held: $held) -> Self {
                    Self(held)
                }
            }

            impl serde::Serialize for $holder {
                fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    serde::Serialize::serialize(&self.0, serializer)
                }
            }

            impl<'de> Deserialize<'de> for $holder {
                fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                    <$held>::deserialize(deserializer).map(Self)
                }
            }

            impl OwnedValues for $holder {
                fn each_value(&mut self, visit: &mut dyn FnMut(&mut Value)) {
                    self.0.each_value(visit);
                }
            }

            impl Drop for $holder {
                fn drop(&mut self) {
                    self.0.drop_values();
                }
            }
        )*
    };
}

deep_json_holder!(DeepJsonObject(JsonObject), DeepJsonValue(Value));

/// A value as the members under undeclared keys of a generated upstream type write it: as `serde_json::Value`
/// writes it while it nests [`SHALLOW`] levels or fewer, and deeper as its compact JSON text, which `serde_json`
/// writers copy as is, so writing it walks no deeper than [`SHALLOW`] levels.
pub(crate) struct ShallowWrite<'a>(pub(crate) &'a Value);

impl serde::Serialize for ShallowWrite<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if nests_within(self.0, SHALLOW) {
            return serde::Serialize::serialize(self.0, serializer);
        }
        let text = compact(self.0).map_err(serde::ser::Error::custom)?;
        let raw = serde_json::value::RawValue::from_string(text).map_err(serde::ser::Error::custom)?;
        serde::Serialize::serialize(&raw, serializer)
    }
}

/// Whether no array or object of `value` sits inside `limit` others.
fn nests_within(value: &Value, limit: usize) -> bool {
    let mut pending = vec![(value, 0_usize)];
    while let Some((value, enclosing)) = pending.pop() {
        let nested = enclosing.saturating_add(1);
        match value {
            Value::Array(items) if enclosing < limit => pending.extend(items.iter().map(|child| (child, nested))),
            Value::Object(object) if enclosing < limit => {
                pending.extend(object.values().map(|child| (child, nested)));
            }
            Value::Array(_) | Value::Object(_) => return false,
            _ => {}
        }
    }
    true
}

/// `left == right` at any nesting: arrays equal item by item, objects equal key by key in any order.
fn equal(left: &Value, right: &Value) -> bool {
    let mut pending = vec![(left, right)];
    while let Some(pair) = pending.pop() {
        match pair {
            (Value::Array(left), Value::Array(right)) => {
                if left.len() != right.len() {
                    return false;
                }
                pending.extend(left.iter().zip(right));
            }
            (Value::Object(left), Value::Object(right)) => {
                if left.len() != right.len() {
                    return false;
                }
                for (key, child) in left {
                    let Some(other) = right.get(key) else {
                        return false;
                    };
                    pending.push((child, other));
                }
            }
            (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => return false,
            (left, right) => {
                if left != right {
                    return false;
                }
            }
        }
    }
    true
}

/// A container [`compact`] or [`debug_value`] is inside, with the children it has left to write.
enum Writing<'a> {
    Array(std::slice::Iter<'a, Value>),
    Object(serde_json::map::Iter<'a>),
}

impl<'a> Writing<'a> {
    /// The next child to write and, in an object, its key.
    fn next_child(&mut self) -> Option<(Option<&'a String>, &'a Value)> {
        match self {
            Self::Array(items) => items.next().map(|child| (None, child)),
            Self::Object(entries) => entries.next().map(|(key, child)| (Some(key), child)),
        }
    }

    fn close(&self) -> &'static str {
        match self {
            Self::Array(_) => "]",
            Self::Object(_) => "}",
        }
    }
}

/// `serde_json::to_string(value)` at any nesting.
fn compact(value: &Value) -> Result<String, serde_json::Error> {
    let mut out: Vec<u8> = Vec::new();
    let mut open: Vec<(Writing<'_>, bool)> = Vec::new();
    let mut next = Some(value);
    loop {
        match next.take() {
            Some(Value::Array(items)) => {
                out.push(b'[');
                open.push((Writing::Array(items.iter()), false));
            }
            Some(Value::Object(object)) => {
                out.push(b'{');
                open.push((Writing::Object(object.iter()), false));
            }
            Some(scalar) => serde_json::to_writer(&mut out, scalar)?,
            None => {}
        }
        let Some((innermost, written)) = open.last_mut() else {
            break;
        };
        match innermost.next_child() {
            Some((key, child)) => {
                if *written {
                    out.push(b',');
                }
                *written = true;
                if let Some(key) = key {
                    serde_json::to_writer(&mut out, key)?;
                    out.push(b':');
                }
                next = Some(child);
            }
            None => {
                out.extend_from_slice(innermost.close().as_bytes());
                open.pop();
            }
        }
    }
    String::from_utf8(out).map_err(serde::ser::Error::custom)
}

/// `Debug` of `serde_json::Value` at any nesting, `{:#?}` included: each container opens with `Array [` or
/// `Object {`, and with `{:#?}` each child sits on its own line, four spaces in per enclosing container.
fn debug_value(value: &Value, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let pretty = formatter.alternate();
    let mut open: Vec<(Writing<'_>, bool)> = Vec::new();
    let mut next = Some(value);
    loop {
        match next.take() {
            Some(Value::Array(items)) => {
                formatter.write_str("Array [")?;
                open.push((Writing::Array(items.iter()), false));
            }
            Some(Value::Object(object)) => {
                formatter.write_str("Object {")?;
                open.push((Writing::Object(object.iter()), false));
            }
            Some(scalar) => std::fmt::Debug::fmt(scalar, formatter)?,
            None => {}
        }
        let depth = open.len();
        let Some((innermost, written)) = open.last_mut() else {
            return Ok(());
        };
        match innermost.next_child() {
            Some((key, child)) => {
                if pretty {
                    formatter.write_str(if *written { ",\n" } else { "\n" })?;
                    indent(formatter, depth)?;
                } else if *written {
                    formatter.write_str(", ")?;
                }
                *written = true;
                if let Some(key) = key {
                    std::fmt::Debug::fmt(key, formatter)?;
                    formatter.write_str(": ")?;
                }
                next = Some(child);
            }
            None => {
                if pretty && *written {
                    formatter.write_str(",\n")?;
                    indent(formatter, depth.saturating_sub(1))?;
                }
                formatter.write_str(innermost.close())?;
                open.pop();
            }
        }
    }
}

/// Four spaces per enclosing container, as `{:#?}` indents a child.
fn indent(formatter: &mut std::fmt::Formatter<'_>, depth: usize) -> std::fmt::Result {
    for _ in 0..depth {
        formatter.write_str("    ")?;
    }
    Ok(())
}

/// A message whose containers nested [`SHALLOW`] levels deep are set aside, so recursive walks such as `serde`
/// decoding run over a bounded `skeleton`. Each container set aside is a string in the skeleton: `marker` and its
/// index in `subtrees`. No string of the message starts with `marker`, so no value reads as one by chance.
pub(crate) struct Detached<'a> {
    pub(crate) skeleton: JsonObject,
    marker: String,
    subtrees: Vec<&'a Value>,
}

/// `object` with its containers nested [`SHALLOW`] levels deep set aside, or `None` when none nests that deep.
pub(crate) fn detach(object: &JsonObject) -> Option<Detached<'_>> {
    let mut deep = false;
    let mut marker = String::from("\u{0}detached:");
    let mut pending: Vec<(&Value, usize)> = object.values().map(|value| (value, 1)).collect();
    let mut strings: Vec<&str> = object.keys().map(String::as_str).collect();
    while let Some((value, depth)) = pending.pop() {
        match value {
            Value::String(text) => strings.push(text),
            Value::Array(items) => {
                deep |= depth >= SHALLOW;
                pending.extend(items.iter().map(|item| (item, depth + 1)));
            }
            Value::Object(members) => {
                deep |= depth >= SHALLOW;
                strings.extend(members.keys().map(String::as_str));
                pending.extend(members.values().map(|member| (member, depth + 1)));
            }
            _ => {}
        }
    }
    if !deep {
        return None;
    }
    while strings.iter().any(|text| text.starts_with(marker.as_str())) {
        marker.push('#');
    }
    let mut detached = Detached {
        skeleton: JsonObject::new(),
        marker,
        subtrees: Vec::new(),
    };
    let skeleton = object
        .iter()
        .map(|(key, value)| (key.clone(), detached.skeleton_of(value, 1)))
        .collect();
    detached.skeleton = skeleton;
    Some(detached)
}

impl<'a> Detached<'a> {
    /// `value` as the skeleton holds it, `depth` levels into the message. The recursion stops at [`SHALLOW`].
    fn skeleton_of(&mut self, value: &'a Value, depth: usize) -> Value {
        match value {
            Value::Array(_) | Value::Object(_) if depth >= SHALLOW => {
                self.subtrees.push(value);
                Value::String(format!("{}{}", self.marker, self.subtrees.len() - 1))
            }
            Value::Array(items) => Value::Array(items.iter().map(|item| self.skeleton_of(item, depth + 1)).collect()),
            Value::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|(key, member)| (key.clone(), self.skeleton_of(member, depth + 1)))
                    .collect(),
            ),
            scalar => scalar.clone(),
        }
    }

    /// Puts a copy of each container the skeleton set aside back where its marker sits in `value`.
    pub(crate) fn reattach(&self, value: &mut Value) {
        let mut pending = vec![value];
        while let Some(value) = pending.pop() {
            match value {
                Value::String(text) => {
                    let subtree = text
                        .strip_prefix(self.marker.as_str())
                        .and_then(|index| index.parse::<usize>().ok())
                        .and_then(|index| self.subtrees.get(index));
                    if let Some(subtree) = subtree {
                        *value = clone_value(subtree);
                    }
                }
                Value::Array(items) => pending.extend(items.iter_mut()),
                Value::Object(members) => pending.extend(members.values_mut()),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "deep_json_tests.rs"]
mod tests;
