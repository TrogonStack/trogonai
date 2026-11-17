//! `Drop`, `Clone`, `PartialEq` and `Debug` for [`PythonJsonValue`], each walking the value with a heap stack
//! instead of recursing. `json.loads` builds values nested as deep as its own C stack allows, and Python frees,
//! copies, compares and prints them without failing, so a deep value here must not overflow the thread's stack
//! where Python returns.

use std::{
    fmt::{self, Write as _},
    slice,
};

use indexmap::{IndexMap, map};

use super::PythonJsonValue;
use crate::python_string::PythonString;

fn is_container(value: &PythonJsonValue) -> bool {
    matches!(value, PythonJsonValue::Array(_) | PythonJsonValue::Object(_))
}

impl Drop for PythonJsonValue {
    /// Moves every nested container onto a heap stack and empties it there, so that no drop goes deeper than one
    /// level. A container whose members hold no container drops them in place.
    fn drop(&mut self) {
        let mut pending = match self {
            Self::Array(items) if items.iter().any(is_container) => std::mem::take(items),
            Self::Object(members) if members.values().any(is_container) => {
                std::mem::take(members).into_values().collect()
            }
            _ => return,
        };
        while let Some(mut value) = pending.pop() {
            match &mut value {
                Self::Array(items) => pending.append(items),
                Self::Object(members) => pending.extend(std::mem::take(members).into_values()),
                _ => {}
            }
        }
    }
}

/// A container the clone is still filling, with the members of the source it has yet to visit and, for an
/// object, the key of the member it visits.
enum Open<'v> {
    Array(Vec<PythonJsonValue>, slice::Iter<'v, PythonJsonValue>),
    Object(
        IndexMap<PythonString, PythonJsonValue>,
        map::Iter<'v, PythonString, PythonJsonValue>,
        PythonString,
    ),
}

impl<'v> Open<'v> {
    /// The copy of `value` when it holds no member, or `None` after opening the container it is.
    fn start(value: &'v PythonJsonValue, open: &mut Vec<Self>) -> Option<PythonJsonValue> {
        match value {
            PythonJsonValue::Null => Some(PythonJsonValue::Null),
            PythonJsonValue::Bool(flag) => Some(PythonJsonValue::Bool(*flag)),
            PythonJsonValue::Number(number) => Some(PythonJsonValue::Number(number.clone())),
            PythonJsonValue::String(text) => Some(PythonJsonValue::String(text.clone())),
            PythonJsonValue::Array(items) => {
                open.push(Self::Array(Vec::with_capacity(items.len()), items.iter()));
                None
            }
            PythonJsonValue::Object(members) => {
                open.push(Self::Object(
                    IndexMap::with_capacity(members.len()),
                    members.iter(),
                    PythonString::new(),
                ));
                None
            }
        }
    }

    /// Stores `child`, the copy of the member [`Open::advance`] returned last.
    fn attach(&mut self, child: PythonJsonValue) {
        match self {
            Self::Array(items, _) => items.push(child),
            Self::Object(members, _, key) => {
                members.insert(std::mem::take(key), child);
            }
        }
    }

    /// The next member to copy, or `None` once every member is stored.
    fn advance(&mut self) -> Option<&'v PythonJsonValue> {
        match self {
            Self::Array(_, rest) => rest.next(),
            Self::Object(_, rest, key) => rest.next().map(|(name, child)| {
                *key = name.clone();
                child
            }),
        }
    }

    fn close(self) -> PythonJsonValue {
        match self {
            Self::Array(items, _) => PythonJsonValue::Array(items),
            Self::Object(members, _, _) => PythonJsonValue::Object(members),
        }
    }
}

impl Clone for PythonJsonValue {
    fn clone(&self) -> Self {
        let mut open = Vec::new();
        let mut built = Open::start(self, &mut open);
        loop {
            if let Some(value) = built.take() {
                let Some(top) = open.last_mut() else {
                    return value;
                };
                top.attach(value);
            }
            built = match open.last_mut().and_then(Open::advance) {
                Some(child) => Open::start(child, &mut open),
                None => open.pop().map(Open::close),
            };
        }
    }
}

impl PartialEq for PythonJsonValue {
    /// The derived equality: arrays equal item by item, objects equal when they hold the same keys with equal
    /// values in any order, as `IndexMap` compares them, and a float by `==`, so `NaN` differs from itself.
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(self, other)];
        while let Some(pair) = pending.pop() {
            match pair {
                (Self::Null, Self::Null) => {}
                (Self::Bool(left), Self::Bool(right)) if left == right => {}
                (Self::Number(left), Self::Number(right)) if left == right => {}
                (Self::String(left), Self::String(right)) if left == right => {}
                (Self::Array(left), Self::Array(right)) if left.len() == right.len() => {
                    pending.extend(left.iter().zip(right));
                }
                (Self::Object(left), Self::Object(right)) if left.len() == right.len() => {
                    for (key, value) in left {
                        let Some(paired) = right.get(key) else {
                            return false;
                        };
                        pending.push((value, paired));
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

/// A step of printing a value.
enum Step<'v> {
    Value(&'v PythonJsonValue),
    Key(&'v PythonString),
    Text(&'static str),
    Indent,
    Dedent,
}

/// Writes to a formatter, starting each line that has text with four spaces per level, as the derived pretty
/// `Debug` indents nested fields.
struct Indented<'f, 'a> {
    formatter: &'f mut fmt::Formatter<'a>,
    level: usize,
    on_newline: bool,
}

impl fmt::Write for Indented<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for line in text.split_inclusive('\n') {
            if self.on_newline {
                for _ in 0..self.level {
                    self.formatter.write_str("    ")?;
                }
            }
            self.on_newline = line.ends_with('\n');
            self.formatter.write_str(line)?;
        }
        Ok(())
    }
}

/// A value without members, printed as the derived `Debug` prints its variant: the name and the one field.
struct Leaf<'n, 'v>(&'n str, &'v dyn fmt::Debug);

impl fmt::Debug for Leaf<'_, '_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple(self.0).field(self.1).finish()
    }
}

/// Writes the leaf variant `name(field)`, compact or with `{:#?}`.
fn leaf(out: &mut Indented<'_, '_>, pretty: bool, name: &str, field: &dyn fmt::Debug) -> fmt::Result {
    let leaf = Leaf(name, field);
    if pretty {
        write!(out, "{leaf:#?}")
    } else {
        write!(out, "{leaf:?}")
    }
}

impl fmt::Debug for PythonJsonValue {
    /// The derived output, compact (`Array([Null, Bool(true)])`) or, with `{:#?}`, one field per line.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pretty = formatter.alternate();
        let mut out = Indented {
            formatter,
            level: 0,
            on_newline: false,
        };
        let mut steps = vec![Step::Value(self)];
        while let Some(step) = steps.pop() {
            match step {
                Step::Value(value) => push_value(value, pretty, &mut steps, &mut out)?,
                Step::Key(key) if pretty => write!(out, "{key:#?}")?,
                Step::Key(key) => write!(out, "{key:?}")?,
                Step::Text(text) => out.write_str(text)?,
                Step::Indent => out.level += 1,
                Step::Dedent => out.level -= 1,
            }
        }
        Ok(())
    }
}

/// Prints a value without members, or pushes the steps that print a container, last step first.
fn push_value<'v>(
    value: &'v PythonJsonValue,
    pretty: bool,
    steps: &mut Vec<Step<'v>>,
    out: &mut Indented<'_, '_>,
) -> fmt::Result {
    let (name, open, close, members) = match value {
        PythonJsonValue::Array(items) => (
            "Array(",
            "[",
            "]",
            items.iter().rev().map(|item| (None, item)).collect::<Vec<_>>(),
        ),
        PythonJsonValue::Object(members) => (
            "Object(",
            "{",
            "}",
            members.iter().rev().map(|(key, member)| (Some(key), member)).collect(),
        ),
        PythonJsonValue::Null => return out.write_str("Null"),
        PythonJsonValue::Bool(flag) => return leaf(out, pretty, "Bool", flag),
        PythonJsonValue::Number(number) => return leaf(out, pretty, "Number", number),
        PythonJsonValue::String(text) => return leaf(out, pretty, "String", text),
    };
    out.write_str(name)?;
    if pretty {
        let filled = !members.is_empty();
        steps.extend([Step::Text(")"), Step::Dedent, Step::Text(",\n"), Step::Text(close)]);
        if filled {
            steps.push(Step::Dedent);
        }
        for (key, member) in members {
            steps.extend([Step::Text(",\n"), Step::Value(member)]);
            if let Some(key) = key {
                steps.extend([Step::Text(": "), Step::Key(key)]);
            }
        }
        if filled {
            steps.extend([Step::Indent, Step::Text("\n")]);
        }
        steps.extend([Step::Text(open), Step::Indent, Step::Text("\n")]);
    } else {
        steps.extend([Step::Text(")"), Step::Text(close)]);
        let last = members.len().saturating_sub(1);
        for (at, (key, member)) in members.into_iter().enumerate() {
            steps.push(Step::Value(member));
            if let Some(key) = key {
                steps.extend([Step::Text(": "), Step::Key(key)]);
            }
            if at != last {
                steps.push(Step::Text(", "));
            }
        }
        steps.push(Step::Text(open));
    }
    Ok(())
}

#[cfg(test)]
#[path = "python_json_value_deep_tests.rs"]
mod tests;
