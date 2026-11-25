//! The Python operations `jsonschema` and `referencing` apply to decoded JSON values, with the `str()` of the
//! exception CPython 3.12 raises when an operand has the wrong type.

use crate::constants::{C_RECURSION_LIMIT, JOINED_REPR_CALLS, PYTHON_INT_MAX_STR_DIGITS, REPR_CALL};
use crate::{
    python_code_point::PythonCodePoint,
    python_json_value::{PythonJsonObject, PythonJsonValue as Value},
    python_re::{decimal_value, is_unicode_space, py_repr},
    python_sort::{PyValue, py_eq, py_lt},
    python_string::PythonString,
    python_value_repr::type_name,
};
use std::borrow::Cow;

/// `value.get(key)`: the raw value, so a JSON `null` member is `Some(Value::Null)`.
pub(super) fn get<'v>(value: &'v Value, key: &str) -> Result<Option<&'v Value>, String> {
    match value {
        Value::Object(map) => Ok(map.get(key)),
        other => Err(format!("'{}' object has no attribute 'get'", type_name(other))),
    }
}

/// `value.get(key)` followed by `is not None`.
pub(super) fn get_some<'v>(value: &'v Value, key: &str) -> Result<Option<&'v Value>, String> {
    Ok(get(value, key)?.filter(|found| !found.is_null()))
}

/// `value.items()`.
pub(super) fn items(value: &Value) -> Result<&PythonJsonObject, String> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(format!("'{}' object has no attribute 'items'", type_name(other))),
    }
}

/// `value.values()`.
pub(super) fn values(value: &Value) -> Result<&PythonJsonObject, String> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(format!("'{}' object has no attribute 'values'", type_name(other))),
    }
}

/// `iter(value)`: list items, mapping keys or the characters of a string.
pub(super) fn iterate(value: &Value) -> Result<Vec<Cow<'_, Value>>, String> {
    match value {
        Value::Array(items) => Ok(items.iter().map(Cow::Borrowed).collect()),
        Value::Object(map) => Ok(map.keys().map(|key| Cow::Owned(Value::String(key.clone()))).collect()),
        Value::String(text) => Ok(text
            .code_points()
            .map(|c| Cow::Owned(Value::String(PythonString::from(c))))
            .collect()),
        other => Err(not_iterable(other)),
    }
}

/// `TypeError` text of `iter(value)` on a value that is not iterable.
pub(super) fn not_iterable(value: &Value) -> String {
    format!("'{}' object is not iterable", type_name(value))
}

/// `TypeError` text of hashing a list or a mapping.
pub(super) fn unhashable(value: &Value) -> Option<String> {
    match value {
        Value::Array(_) | Value::Object(_) => Some(format!("unhashable type: '{}'", type_name(value))),
        _ => None,
    }
}

/// `item in container`.
pub(super) fn contains(container: &Value, item: &Value) -> Result<bool, String> {
    match container {
        Value::Object(map) => {
            if let Some(error) = unhashable(item) {
                return Err(error);
            }
            Ok(item.as_python_str().is_some_and(|key| map.contains_key(key)))
        }
        Value::Array(items) => {
            let wanted = PyValue::from_json(item);
            Ok(items.iter().any(|each| py_eq(&PyValue::from_json(each), &wanted)))
        }
        Value::String(text) => match item {
            Value::String(needle) => Ok(text.contains(needle)),
            other => Err(format!(
                "'in <string>' requires string as left operand, not {}",
                type_name(other)
            )),
        },
        other => Err(format!("argument of type '{}' is not iterable", type_name(other))),
    }
}

/// `key in container` for a string key.
pub(super) fn has(container: &Value, key: &str) -> Result<bool, String> {
    contains(container, &Value::from(key))
}

/// `name in container` for a mapping key.
pub(super) fn has_name(container: &Value, name: &PythonString) -> Result<bool, String> {
    contains(container, &Value::String(name.clone()))
}

/// `len(value)`.
pub(super) fn len(value: &Value) -> Result<usize, String> {
    match value {
        Value::Array(items) => Ok(items.len()),
        Value::Object(map) => Ok(map.len()),
        Value::String(text) => Ok(text.len()),
        other => Err(format!("object of type '{}' has no len()", type_name(other))),
    }
}

/// A rich comparison operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Op {
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// `TypeError` text of an unsupported comparison.
pub(super) fn unorderable(op: Op, left: &str, right: &str) -> String {
    format!(
        "'{}' not supported between instances of '{left}' and '{right}'",
        op.symbol()
    )
}

/// `left OP right` where `left` is an `int` or a `float`.
pub(super) fn compare(left: &Value, op: Op, right: &Value) -> Result<bool, String> {
    if !matches!(right, Value::Number(_) | Value::Bool(_)) {
        return Err(unorderable(op, type_name(left), type_name(right)));
    }
    let (left, right) = (PyValue::from_json(left), PyValue::from_json(right));
    let less = |a: &PyValue, b: &PyValue| py_lt(a, b).unwrap_or(false);
    Ok(match op {
        Op::Lt => less(&left, &right),
        Op::Le => less(&left, &right) || py_eq(&left, &right),
        Op::Gt => less(&right, &left),
        Op::Ge => less(&right, &left) || py_eq(&left, &right),
    })
}

/// `count OP right` for a Python `int` such as a `len()`.
pub(super) fn compare_count(count: usize, op: Op, right: &Value) -> Result<bool, String> {
    compare(&Value::from(count), op, right)
}

/// `value == number` for a small Python `int`.
pub(super) fn equals_int(value: &Value, number: i128) -> bool {
    py_eq(&PyValue::from_json(value), &PyValue::Int(number))
}

/// A Python `int` parsed from a JSON pointer segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Index {
    /// Fits an `i128`.
    Small(i128),
    /// Too large for any sequence, so the lookup always misses.
    Huge,
}

/// The `ValueError` of `int(text)` for a text that is no integer literal, its repr cut to 200 characters.
fn invalid_int(text: &PythonString) -> String {
    let shown: String = py_repr(text).chars().take(200).collect();
    format!("invalid literal for int() with base 10: {shown}")
}

/// `int(text)` for any Python `str`. A lone surrogate is neither a digit nor a space, so a text holding one is
/// always an invalid literal.
pub(super) fn parse_python_int(text: &PythonString) -> Result<Index, String> {
    match text.as_str() {
        Some(text) => parse_int(text),
        None => Err(invalid_int(text)),
    }
}

/// `int(text)`.
pub(super) fn parse_int(text: &str) -> Result<Index, String> {
    let invalid = || invalid_int(&text.into());
    let mut ascii = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else if is_unicode_space(c) {
            ascii.push(' ');
        } else if let Some(digit) = decimal_value(c) {
            ascii.push(char::from(b'0' + digit));
        } else {
            return Err(invalid());
        }
    }
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r');
    let body = ascii.trim_start_matches(is_space);
    let (negative, body) = match body.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, body.strip_prefix('+').unwrap_or(body)),
    };
    let run_end = body
        .find(|c: char| !(c.is_ascii_digit() || c == '_'))
        .unwrap_or(body.len());
    let (run, rest) = body.split_at(run_end);
    let syntax_ok = !run.is_empty()
        && !run.starts_with('_')
        && !run.ends_with('_')
        && !run.contains("__")
        && rest.trim_start_matches(is_space).is_empty();
    if !syntax_ok {
        return Err(invalid());
    }
    let digits = run.chars().filter(char::is_ascii_digit).count();
    if digits > PYTHON_INT_MAX_STR_DIGITS {
        return Err(format!(
            "Exceeds the limit ({PYTHON_INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
        ));
    }
    let mut value: i128 = 0;
    for digit in run.chars().filter_map(|c| c.to_digit(10)) {
        match value.checked_mul(10).and_then(|v| v.checked_add(i128::from(digit))) {
            Some(next) => value = next,
            None => return Ok(Index::Huge),
        }
    }
    Ok(Index::Small(if negative { -value } else { value }))
}

/// `sequence[index]` with Python's negative indexing, `None` when out of range.
pub(super) fn position(index: Index, length: usize) -> Option<usize> {
    let Index::Small(index) = index else {
        return None;
    };
    let length = i128::try_from(length).ok()?;
    let resolved = if index < 0 { index + length } else { index };
    if (0..length).contains(&resolved) {
        usize::try_from(resolved).ok()
    } else {
        None
    }
}

/// `", ".join(repr(each) for each in values)` with `held` C recursion checks under way.
pub(super) fn join_reprs<'v>(values: impl IntoIterator<Item = &'v Value>, held: usize) -> Result<String, String> {
    let mut parts = Vec::new();
    for value in values {
        parts.push(shown_under(value, held + JOINED_REPR_CALLS)?);
    }
    Ok(parts.join(", "))
}

/// `jsonschema._utils.extras_msg` with `held` C recursion checks under way.
pub(super) fn extras_msg<'v>(
    extras: impl IntoIterator<Item = &'v Value>,
    held: usize,
) -> Result<(String, &'static str), String> {
    let extras: Vec<&Value> = extras.into_iter().collect();
    let verb = if extras.len() == 1 { "was" } else { "were" };
    Ok((join_reprs(extras, held)?, verb))
}

/// The deepest `PyObject_Repr` call `repr(value)` makes, counting its own: 1 for a scalar or an empty container,
/// one more than its deepest item for any other container.
pub(super) fn repr_depth(value: &Value) -> usize {
    let mut deepest = 0;
    let mut pending = vec![(value, 1_usize)];
    while let Some((value, depth)) = pending.pop() {
        deepest = deepest.max(depth);
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(map) => pending.extend(map.values().map(|item| (item, depth + 1))),
            _ => {}
        }
    }
    deepest
}

/// `{value!r}` in a keyword's message with `held` C recursion checks under way: CPython raises `RecursionError`
/// instead when the checks of the nested reprs would pass [`C_RECURSION_LIMIT`]. Python frames run inside the
/// evaluation loop that holds them, so neither the nesting of the schema nor the keyword moves it.
pub(super) fn shown(value: &Value, held: usize) -> Result<String, String> {
    shown_under(value, held)
}

/// `repr(value)` called in a keyword's own frame with `held` C recursion checks under way.
pub(super) fn repr_called(value: &Value, held: usize) -> Result<String, String> {
    shown_under(value, held + REPR_CALL)
}

/// The repr of `value` made with `held` C recursion checks under way.
fn shown_under(value: &Value, held: usize) -> Result<String, String> {
    if held + repr_depth(value) > C_RECURSION_LIMIT {
        return Err("maximum recursion depth exceeded while getting the repr of an object".to_owned());
    }
    Ok(crate::python_value_repr::repr(value))
}

/// `"|".join(value)`.
pub(super) fn join_patterns(value: &Value) -> Result<PythonString, String> {
    let parts: Vec<PythonString> = match value {
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                match item {
                    Value::String(text) => parts.push(text.clone()),
                    other => {
                        return Err(format!(
                            "sequence item {index}: expected str instance, {} found",
                            type_name(other)
                        ));
                    }
                }
            }
            parts
        }
        Value::Object(map) => map.keys().cloned().collect(),
        Value::String(text) => text.code_points().map(PythonString::from).collect(),
        _ => return Err("can only join an iterable".to_owned()),
    };
    let mut joined = PythonString::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            joined.push(PythonCodePoint::from('|'));
        }
        joined.extend(part.code_points());
    }
    Ok(joined)
}

/// `sorted(values)` of strings or other orderable values, with Python's `TypeError` on mixed types.
pub(super) fn sorted_values(values: Vec<Cow<'_, Value>>) -> Result<Vec<Cow<'_, Value>>, String> {
    let keys: Vec<PyValue> = values.iter().map(|value| PyValue::from_json(value)).collect();
    let mut trace = Vec::new();
    let Ok(order) = crate::python_sort::sorted_indices(&keys, Some(&mut trace)) else {
        let names = trace
            .last()
            .and_then(|&(left, right)| Some((values.get(left)?, values.get(right)?)));
        let (left, right) = names.map_or(("", ""), |(left, right)| (type_name(left), type_name(right)));
        return Err(unorderable(Op::Lt, left, right));
    };
    let mut slots: Vec<Option<Cow<'_, Value>>> = values.into_iter().map(Some).collect();
    Ok(order
        .into_iter()
        .filter_map(|index| slots.get_mut(index).and_then(Option::take))
        .collect())
}

/// Python truthiness of a decoded JSON value.
pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.to_f64() != 0.0,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}
