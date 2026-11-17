//! JavaScript arrays inside a settings value while a TypeScript-dialect resolve merges it.
//!
//! lodash `merge` leaves holes and `undefined` elements in arrays and gives arrays named properties, which a JSON
//! value cannot hold. A merge keeps them in band with the [`ArrayMark`] of the resolve, a run of holes staying one
//! value however long it is, as V8 keeps a sparse array, and [`settle`] turns them into what `JSON.stringify` prints
//! once the resolve is done: each hole or `undefined` element as `null`, and no named property.

use std::mem;

use serde_json::{Map, Value};

use crate::{
    constants::{JS_MAX_ARRAY_LENGTH, JS_MAX_EXACT_INTEGER, V8_MAX_STRING_LENGTH},
    js_array_mark::ArrayMark,
    js_object_order::{array_index, js_key_order},
    settings_json::js_number,
    settings_numbers::{js_value, same_value_zero},
    settings_schema_ports::string_to_number,
    settings_strings::js_length,
};

/// A string a JavaScript operation would build is longer than V8 holds, which throws
/// `RangeError: Invalid string length`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StringTooLong;

/// What assigning `length` to an array threw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LengthError {
    /// `RangeError: Invalid array length`: the value is not a whole number from 0 to 2^32 - 1.
    InvalidArrayLength,
    /// `RangeError: Invalid string length`: converting the value to a number joins an array longer than V8 holds.
    InvalidStringLength,
}

/// One member lodash reads from an array it merges from, in the order `keysIn` lists them.
pub(crate) enum Member<'a> {
    Element(u64, &'a Value),
    /// The holes or `undefined` elements from the end of the elements before them up to `end`.
    Undefined {
        end: u64,
    },
    Named(&'a String, &'a Value),
}

/// The elements of `items`, without the object after them that holds its named properties.
pub(crate) fn elements(items: &[Value], mark: ArrayMark) -> &[Value] {
    match items.split_last() {
        Some((last, rest)) if mark.is_props(last) => rest,
        _ => items,
    }
}

/// The named properties of `items`, in the order they were set.
pub(crate) fn named(items: &[Value], mark: ArrayMark) -> impl Iterator<Item = (&String, &Value)> {
    items
        .last()
        .filter(|last| mark.is_props(last))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(move |(key, _)| !mark.is_props_key(key))
}

fn span(item: &Value, mark: ArrayMark) -> u64 {
    mark.run_length(item).unwrap_or(1)
}

/// The `length` of `items`.
pub(crate) fn length(items: &[Value], mark: ArrayMark) -> u64 {
    elements(items, mark)
        .iter()
        .fold(0, |total: u64, item| total.saturating_add(span(item, mark)))
}

/// The members of a source array: its elements by index, then its named properties.
pub(crate) fn members(items: &[Value], mark: ArrayMark) -> Vec<Member<'_>> {
    let mut members = Vec::new();
    let mut start = 0u64;
    for item in elements(items, mark) {
        match mark.run_length(item) {
            Some(run) => {
                let end = start.saturating_add(run);
                members.push(Member::Undefined { end });
                start = end;
            }
            None => {
                members.push(Member::Element(start, item));
                start = start.saturating_add(1);
            }
        }
    }
    members.extend(named(items, mark).map(|(key, value)| Member::Named(key, value)));
    members
}

/// Where an index of an array falls.
enum Place {
    Element(usize),
    InRun { at: usize, offset: u64, run: u64 },
    Beyond,
}

/// An array a merge writes into.
pub(crate) struct JsArray<'v> {
    items: &'v mut Vec<Value>,
    mark: ArrayMark,
    /// Whether no element is a run, so each index is its own element.
    dense: bool,
}

impl<'v> JsArray<'v> {
    pub(crate) fn new(items: &'v mut Vec<Value>, mark: ArrayMark) -> Self {
        let dense = elements(items, mark).iter().all(|item| mark.run_length(item).is_none());
        Self { items, mark, dense }
    }

    fn element_count(&self) -> usize {
        elements(self.items, self.mark).len()
    }

    pub(crate) fn length(&self) -> u64 {
        if self.dense {
            self.element_count() as u64
        } else {
            length(self.items, self.mark)
        }
    }

    fn place(&self, index: u64) -> Place {
        if self.dense {
            return usize::try_from(index)
                .ok()
                .filter(|at| *at < self.element_count())
                .map_or(Place::Beyond, Place::Element);
        }
        let mut start = 0u64;
        for (at, item) in elements(self.items, self.mark).iter().enumerate() {
            let end = start.saturating_add(span(item, self.mark));
            if index < end {
                return match self.mark.run_length(item) {
                    Some(run) => Place::InRun {
                        at,
                        offset: index.saturating_sub(start),
                        run,
                    },
                    None => Place::Element(at),
                };
            }
            start = end;
        }
        Place::Beyond
    }

    /// The element at `index`, unless the array holds a hole or `undefined` there or ends before it.
    pub(crate) fn get(&self, index: u64) -> Option<&Value> {
        match self.place(index) {
            Place::Element(at) => self.items.get(at),
            Place::InRun { .. } | Place::Beyond => None,
        }
    }

    pub(crate) fn get_mut(&mut self, index: u64) -> Option<&mut Value> {
        match self.place(index) {
            Place::Element(at) => self.items.get_mut(at),
            Place::InRun { .. } | Place::Beyond => None,
        }
    }

    /// `array[index] = value`, for a value other than `undefined`.
    pub(crate) fn set(&mut self, index: u64, value: Value) {
        match self.place(index) {
            Place::Element(at) => {
                if let Some(slot) = self.items.get_mut(at) {
                    *slot = value;
                }
            }
            Place::InRun { at, offset, run } => {
                let after = run.saturating_sub(offset).saturating_sub(1);
                let mut parts = Vec::with_capacity(3);
                if offset > 0 {
                    parts.push(self.mark.run(offset));
                }
                parts.push(value);
                if after > 0 {
                    parts.push(self.mark.run(after));
                }
                self.items.splice(at..=at, parts).for_each(drop);
            }
            Place::Beyond => {
                let gap = index.saturating_sub(self.length());
                let at = self.element_count();
                let mut parts = Vec::with_capacity(2);
                if gap > 0 {
                    parts.push(self.mark.run(gap));
                    self.dense = false;
                }
                parts.push(value);
                self.items.splice(at..at, parts).for_each(drop);
            }
        }
    }

    /// `undefined` assigned at every index below `end`, which lodash writes only where the array holds no element:
    /// past its end.
    pub(crate) fn fill_undefined(&mut self, end: u64) {
        if end > self.length() {
            self.set_length(end);
        }
    }

    /// `array.length = length`, for a valid length.
    pub(crate) fn set_length(&mut self, length: u64) {
        let current = self.length();
        if length > current {
            let at = self.element_count();
            self.items.insert(at, self.mark.run(length.saturating_sub(current)));
            self.dense = false;
            return;
        }
        let end = self.element_count();
        match self.place(length) {
            Place::Element(at) | Place::InRun { at, offset: 0, .. } => {
                self.items.drain(at..end).for_each(drop);
            }
            Place::InRun { at, offset, .. } => {
                self.items.drain(at.saturating_add(1)..end).for_each(drop);
                if let Some(slot) = self.items.get_mut(at) {
                    *slot = self.mark.run(offset);
                }
            }
            Place::Beyond => {}
        }
        let mark = self.mark;
        self.dense = elements(self.items, mark)
            .iter()
            .all(|item| mark.run_length(item).is_none());
    }

    fn holder_mut(&mut self) -> Option<&mut Map<String, Value>> {
        let mark = self.mark;
        match self.items.last_mut() {
            Some(last) if mark.is_props(last) => last.as_object_mut(),
            _ => None,
        }
    }

    /// The named property `name`.
    pub(crate) fn prop(&self, name: &str) -> Option<&Value> {
        named(self.items, self.mark)
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }

    pub(crate) fn prop_mut(&mut self, name: &str) -> Option<&mut Value> {
        self.holder_mut()?.get_mut(name)
    }

    /// `array[name] = value` for a name that is neither an index nor `length`.
    pub(crate) fn set_prop(&mut self, name: &str, value: Value) {
        if self.holder_mut().is_none() {
            self.items.push(self.mark.props());
        }
        if let Some(holder) = self.holder_mut() {
            holder.insert(name.to_owned(), value);
        }
    }
}

/// `copyArray` upstream for an array-like object lodash merges an array into: `None` when `object` is not array-like
/// (its `length` is not a whole number from 0 to 2^53 - 1), and [`LengthError::InvalidArrayLength`] when
/// `Array(length)` throws.
pub(crate) fn copy_array_like(object: &Map<String, Value>, mark: ArrayMark) -> Result<Option<Vec<Value>>, LengthError> {
    let Some(Value::Number(number)) = object.get("length") else {
        return Ok(None);
    };
    let Some(length) = js_value(number)
        .filter(|length| length.fract() == 0.0 && *length > -1.0 && *length <= (JS_MAX_EXACT_INTEGER - 1) as f64)
    else {
        return Ok(None);
    };
    if length > JS_MAX_ARRAY_LENGTH as f64 {
        return Err(LengthError::InvalidArrayLength);
    }
    let length = length as u64;
    let mut copied = Vec::new();
    let mut next = 0u64;
    for (key, value) in js_key_order(object) {
        let Some(index) = array_index(key).map(u64::from).filter(|index| *index < length) else {
            continue;
        };
        if index > next {
            copied.push(mark.run(index - next));
        }
        copied.push(value.clone());
        next = index.saturating_add(1);
    }
    if length > next {
        copied.push(mark.run(length - next));
    }
    Ok(Some(copied))
}

/// The length assigning `value` to `length` sets: `ToNumber(value)`, which must be a whole number from 0 to
/// 2^32 - 1.
pub(crate) fn array_length(value: &Value, mark: ArrayMark) -> Result<u64, LengthError> {
    let number = match value {
        Value::Array(items) if join_length(items, mark) > V8_MAX_STRING_LENGTH => {
            return Err(LengthError::InvalidStringLength);
        }
        Value::Array(items) => joined_number(items, mark),
        Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
        other => to_number_of_text(other, mark),
    };
    number
        .filter(|number| number.fract() == 0.0 && (0.0..=JS_MAX_ARRAY_LENGTH as f64).contains(number))
        .map(|number| number as u64)
        .ok_or(LengthError::InvalidArrayLength)
}

/// `Number(String(value))` for a value an array join prints, `None` standing for `NaN`.
fn to_number_of_text(value: &Value, mark: ArrayMark) -> Option<f64> {
    match value {
        Value::Null => Some(0.0),
        Value::Bool(_) | Value::Object(_) => None,
        Value::Number(number) => js_value(number),
        Value::String(text) => string_to_number(text),
        Value::Array(items) => joined_number(items, mark),
    }
}

/// `Number(items.join())` for a join that fits a V8 string: only an array of at most one element prints without a
/// comma.
fn joined_number(items: &[Value], mark: ArrayMark) -> Option<f64> {
    match elements(items, mark) {
        [] => Some(0.0),
        [single] => match mark.run_length(single) {
            Some(1) => Some(0.0),
            Some(_) => None,
            None => to_number_of_text(single, mark),
        },
        _ => None,
    }
}

/// The UTF-16 length of `items.join()`.
fn join_length(items: &[Value], mark: ArrayMark) -> u64 {
    let mut total = length(items, mark).saturating_sub(1);
    for item in elements(items, mark) {
        let printed = match item {
            Value::Null => 0,
            Value::Bool(true) => 4,
            Value::Bool(false) => 5,
            Value::Number(number) => js_number(number).len() as u64,
            Value::String(text) => js_length(text) as u64,
            Value::Array(nested) => join_length(nested, mark),
            Value::Object(_) if mark.run_length(item).is_some() => 0,
            Value::Object(_) => "[object Object]".len() as u64,
        };
        total = total.saturating_add(printed);
    }
    total
}

fn is_primitive(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

/// `Xe([...left, ...right])` upstream: the elements of both arrays, holes read as `undefined`, with a `Set` keeping
/// one of each primitive and every array or object; named properties are not elements and drop.
pub(crate) fn union(left: &[Value], right: &[Value], mark: ArrayMark) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(left.len() + right.len());
    let mut has_undefined = false;
    for value in elements(left, mark).iter().chain(elements(right, mark)) {
        if mark.run_length(value).is_some() {
            if !has_undefined {
                merged.push(mark.run(1));
                has_undefined = true;
            }
            continue;
        }
        if is_primitive(value) && merged.iter().any(|seen| same_value_zero(seen, value)) {
            continue;
        }
        merged.push(value.clone());
    }
    merged
}

/// A copy of `value` as lodash merges it into a fresh array or object: without the `__proto__` arrays and objects
/// `safeGet` hides, except inside a value under a key in `kept`, which the settings customizer answers whole.
pub(crate) fn fresh_copy(value: &Value, kept: &[String], mark: ArrayMark) -> Value {
    let mut copy = value.clone();
    let mut pending = vec![&mut copy];
    while let Some(value) = pending.pop() {
        let entries = match value {
            Value::Array(items) => {
                let count = elements(items, mark).len();
                let (listed, holder) = items.split_at_mut(count);
                pending.extend(listed);
                match holder.first_mut() {
                    Some(Value::Object(entries)) => entries,
                    _ => continue,
                }
            }
            Value::Object(entries) => entries,
            _ => continue,
        };
        entries.retain(|key, value| key != "__proto__" || is_primitive(value));
        pending.extend(
            entries
                .iter_mut()
                .filter(|(key, _)| !kept.iter().any(|name| name == *key))
                .map(|(_, value)| value),
        );
    }
    copy
}

/// Turns every run of `values` into that many `null`s and drops every holder of named properties, as
/// `JSON.stringify` prints them. Fails when the runs alone print longer than V8 holds, before it writes any.
pub(crate) fn settle(values: &mut [Value], mark: ArrayMark) -> Result<(), StringTooLong> {
    let mut holes = 0u64;
    let mut pending: Vec<&Value> = values.iter().collect();
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => {
                for item in elements(items, mark) {
                    match mark.run_length(item) {
                        Some(run) => holes = holes.saturating_add(run),
                        None => pending.push(item),
                    }
                }
            }
            Value::Object(entries) => pending.extend(entries.values()),
            _ => {}
        }
    }
    if holes.saturating_mul("null".len() as u64) > V8_MAX_STRING_LENGTH {
        return Err(StringTooLong);
    }
    let mut pending: Vec<&mut Value> = values.iter_mut().collect();
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => {
                if items
                    .iter()
                    .any(|item| mark.run_length(item).is_some() || mark.is_props(item))
                {
                    let count = elements(items, mark).len();
                    let mut settled = Vec::with_capacity(count);
                    for item in mem::take(items).into_iter().take(count) {
                        match mark.run_length(&item) {
                            Some(run) => {
                                settled.extend((0..run).map(|_| Value::Null));
                            }
                            None => settled.push(item),
                        }
                    }
                    *items = settled;
                }
                pending.extend(items.iter_mut());
            }
            Value::Object(entries) => pending.extend(entries.values_mut()),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "js_array_tests.rs"]
mod tests;
