//! Python `repr` and `type(value).__name__` of the value `json.loads` reads from JSON text.

use crate::{
    mcp_wire_ids::{marker_str, marker_text},
    python_json::float_repr,
    python_json_load::drop_python_deep,
    python_json_value::{PythonJsonValue as Value, PythonNumber},
    python_re::py_repr,
    python_string::PythonString,
};

/// Python `repr` of the value `json.loads` reads from this JSON: `None`, `True`, `False`, single-quoted strings,
/// `{key: value}` mappings in insertion order and floats spelled as Python spells them. A request id too big for
/// 64 bits stands in the message as a marker string and is spelled as the integer Python read.
pub(crate) fn repr(value: &Value) -> String {
    repr_nested(value, false).0
}

/// [`repr`] of a `serde_json::Value`, as `json.loads` would read it.
pub(crate) fn repr_json(value: &serde_json::Value) -> String {
    let value = Value::from(value);
    let text = repr(&value);
    drop_python_deep(value);
    text
}

/// A container [`repr_nested`] is still writing: the members left, and whether one was written.
enum Open<'v> {
    List(std::slice::Iter<'v, Value>, bool),
    Dict(std::vec::IntoIter<(&'v PythonString, &'v Value)>, bool),
}

impl<'v> Open<'v> {
    /// The next member to write, after its separator and, in a mapping, its key.
    fn next_member(&mut self, text: &mut String) -> Option<&'v Value> {
        match self {
            Self::List(items, written) => {
                let member = items.next()?;
                separate(written, text);
                Some(member)
            }
            Self::Dict(entries, written) => {
                let (key, member) = entries.next()?;
                separate(written, text);
                text.push_str(&py_repr(key));
                text.push_str(": ");
                Some(member)
            }
        }
    }

    fn closing(&self) -> char {
        match self {
            Self::List(..) => ']',
            Self::Dict(..) => '}',
        }
    }
}

/// Writes `, ` before every member but the first.
fn separate(written: &mut bool, text: &mut String) {
    if std::mem::replace(written, true) {
        text.push_str(", ");
    }
}

/// [`repr`] with the mapping entries sorted by key when `sort_dicts`, as `pprint` writes them, and how deep the
/// `repr` nests: 1 for a scalar or an empty container, one more than its deepest member for any other container.
/// It writes the containers under way from a stack, so no nesting recurses here.
pub(crate) fn repr_nested(value: &Value, sort_dicts: bool) -> (String, usize) {
    let mut text = String::new();
    let mut open: Vec<Open<'_>> = Vec::new();
    let mut deepest = 0;
    let mut next = Some(value);
    loop {
        if let Some(value) = next.take() {
            deepest = deepest.max(open.len() + 1);
            match value {
                Value::Null => text.push_str("None"),
                Value::Bool(true) => text.push_str("True"),
                Value::Bool(false) => text.push_str("False"),
                Value::Number(number) => text.push_str(&number_repr(number)),
                Value::String(string) => match string.as_str().and_then(marker_str) {
                    Some(integer) => text.push_str(&integer),
                    None => text.push_str(&py_repr(string)),
                },
                Value::Array(items) => {
                    text.push('[');
                    open.push(Open::List(items.iter(), false));
                }
                Value::Object(object) => {
                    let mut entries: Vec<(&PythonString, &Value)> = object.iter().collect();
                    if sort_dicts {
                        entries.sort_by(|left, right| left.0.cmp(right.0));
                    }
                    text.push('{');
                    open.push(Open::Dict(entries.into_iter(), false));
                }
            }
        }
        let Some(top) = open.last_mut() else {
            return (text, deepest);
        };
        next = top.next_member(&mut text);
        if next.is_none() {
            text.push(top.closing());
            open.pop();
        }
    }
}

/// Python `repr` of a JSON number: an `int` when it has no fraction or exponent, a `float` otherwise. A
/// non-finite `float` reprs as `nan`, `inf` or `-inf`, not as JSON spells it.
fn number_repr(number: &PythonNumber) -> String {
    match number {
        PythonNumber::Int(integer) => integer.to_string(),
        PythonNumber::Float(float) if float.is_nan() => "nan".to_owned(),
        PythonNumber::Float(float) if float.is_infinite() => if *float > 0.0 { "inf" } else { "-inf" }.to_owned(),
        PythonNumber::Float(float) => float_repr(*float),
    }
}

/// `type(value).__name__` of the value `json.loads` reads from this JSON.
pub(crate) fn type_name(value: &Value) -> &'static str {
    if value.as_str().and_then(marker_str).is_some() {
        return "int";
    }
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// [`type_name`] of a `serde_json::Value`.
pub(crate) fn json_type_name(value: &serde_json::Value) -> &'static str {
    if marker_text(value).is_some() {
        return "int";
    }
    match value {
        serde_json::Value::Null => "NoneType",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(number) if number.is_f64() => "float",
        serde_json::Value::Number(_) => "int",
        serde_json::Value::String(_) => "str",
        serde_json::Value::Array(_) => "list",
        serde_json::Value::Object(_) => "dict",
    }
}

#[cfg(test)]
#[path = "python_value_repr_tests.rs"]
mod tests;
