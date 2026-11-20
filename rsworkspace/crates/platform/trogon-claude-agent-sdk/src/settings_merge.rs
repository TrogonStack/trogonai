//! lodash `mergeWith` with the settings customizer (`ao` upstream).
//!
//! The merge writes into the destination as it goes. A value the destination already holds as an array or object is
//! merged into in place, while a value it does not hold is written as a fresh copy of the source value.
//!
//! lodash reads each member through `safeGet`, which answers `undefined` for `__proto__`: an array or object under
//! that key merges nothing and is never entered, at any level of a fresh copy too, while a string, number, boolean
//! or `null` there is written as an own member. It walks an object's members as `for...in` does, array indices
//! first, and an array's as `keysIn` lists them: every index, holes included, then its named properties.
//!
//! An object merged onto an array sets elements, `length` and named properties of the array, and an array merged
//! onto an object with a valid `length` merges into `copyArray` of that object. Both leave holes and named
//! properties, which the merge keeps in band as [`js_array`](crate::js_array) describes.
//!
//! The merge recurses once per level of the source, so the depth it reaches is the depth of the settings documents
//! it merges, which [`settings_depth`](crate::settings_depth) bounds for the TypeScript dialect.

use serde_json::{Map, Value};

use crate::{
    js_array::{JsArray, LengthError, Member, array_length, copy_array_like, fresh_copy, members, union},
    js_array_mark::ArrayMark,
    js_object_order::{array_index, js_key_order},
    settings_merge_spec::MergeSpec,
    settings_resolve_error::ResolveSettingsError,
    settings_strings::array_mark,
};

impl From<LengthError> for ResolveSettingsError {
    /// What `resolveSettings` rejects with where nothing catches the `RangeError` a length raises.
    fn from(error: LengthError) -> Self {
        match error {
            LengthError::InvalidArrayLength => Self::InvalidArrayLength,
            LengthError::InvalidStringLength => Self::InvalidStringLength,
        }
    }
}

/// A source array or object a merge reads members from.
#[derive(Clone, Copy)]
enum Source<'a> {
    Object(&'a Map<String, Value>),
    Array(&'a [Value]),
}

impl<'a> Source<'a> {
    fn members(self, mark: ArrayMark) -> Vec<Member<'a>> {
        match self {
            Self::Object(entries) => js_key_order(entries)
                .into_iter()
                .map(|(name, value)| Member::Named(name, value))
                .collect(),
            Self::Array(items) => members(items, mark),
        }
    }
}

/// The key a merge reads and writes a member under: for an array, an index, its `length` or a named property.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Key<'a> {
    Index(u64),
    Length,
    Name(&'a str),
}

impl Key<'_> {
    /// Whether `keys` lists this key, which only a named key can be.
    fn listed(self, keys: &[String]) -> bool {
        matches!(self, Self::Name(name) if keys.iter().any(|listed| listed == name))
    }
}

/// An array or object of the destination a merge writes members into.
enum Target<'v> {
    Object(&'v mut Map<String, Value>),
    Array(JsArray<'v>),
}

impl Target<'_> {
    fn key<'a>(&self, name: &'a str) -> Key<'a> {
        match self {
            Self::Object(_) => Key::Name(name),
            Self::Array(_) if name == "length" => Key::Length,
            Self::Array(_) => array_index(name).map_or(Key::Name(name), |index| Key::Index(u64::from(index))),
        }
    }

    /// The member under `key` as `safeGet` reads it: never under `__proto__`, and no element in a hole.
    fn get(&self, key: Key<'_>) -> Option<&Value> {
        match (self, key) {
            (_, Key::Name("__proto__") | Key::Length) | (Self::Object(_), Key::Index(_)) => None,
            (Self::Object(entries), Key::Name(name)) => entries.get(name),
            (Self::Array(array), Key::Index(index)) => array.get(index),
            (Self::Array(array), Key::Name(name)) => array.prop(name),
        }
    }

    fn get_mut(&mut self, key: Key<'_>) -> Option<&mut Value> {
        match (self, key) {
            (_, Key::Name("__proto__") | Key::Length) | (Self::Object(_), Key::Index(_)) => None,
            (Self::Object(entries), Key::Name(name)) => entries.get_mut(name),
            (Self::Array(array), Key::Index(index)) => array.get_mut(index),
            (Self::Array(array), Key::Name(name)) => array.prop_mut(name),
        }
    }

    /// `target[key] = value`, which for the `length` of an array sets `ToNumber(value)` as its length. An object
    /// only ever takes a named key: [`Target::key`] names every member of an object, and lodash `baseMergeDeep` merges
    /// a source array onto a held object through `copyArray`, so an object never takes the elements of an array.
    fn write(&mut self, key: Key<'_>, value: Value, mark: ArrayMark) -> Result<(), LengthError> {
        match self {
            Self::Object(entries) => {
                if let Key::Name(name) = key {
                    entries.insert(name.to_owned(), value);
                }
            }
            Self::Array(array) => match key {
                Key::Index(index) => array.set(index, value),
                Key::Length => array.set_length(array_length(&value, mark)?),
                Key::Name(name) => array.set_prop(name, value),
            },
        }
        Ok(())
    }

    /// `target[key] = undefined` for the holes or `undefined` elements of a source array up to `end`, which lodash
    /// writes only where the array holds no element. The target is the array the source array merges into.
    fn write_undefined(&mut self, end: u64) {
        if let Self::Array(array) = self {
            array.fill_undefined(end);
        }
    }
}

fn container(value: &Value) -> Option<Source<'_>> {
    match value {
        Value::Object(entries) => Some(Source::Object(entries)),
        Value::Array(items) => Some(Source::Array(items)),
        _ => None,
    }
}

/// One merge: the settings customizer's rules and the mark of the arrays it writes.
struct Merge<'r> {
    rules: &'r MergeSpec,
    mark: ArrayMark,
}

/// Merges `source` into `destination` as lodash `mergeWith` with the settings customizer does, keeping what it
/// wrote before it stops. A rejection is the `RangeError` lodash raises for a `length` it cannot set.
pub(crate) fn merge_settings(
    rules: &MergeSpec,
    destination: &mut Map<String, Value>,
    source: &Map<String, Value>,
) -> Result<(), ResolveSettingsError> {
    let merge = Merge {
        rules,
        mark: array_mark(),
    };
    merge
        .merge_members(&mut Target::Object(destination), Source::Object(source))
        .map_err(ResolveSettingsError::from)
}

impl Merge<'_> {
    /// Merges the members of `source`, an array or object the merge entered, into `target` in their order.
    fn merge_members(&self, target: &mut Target<'_>, source: Source<'_>) -> Result<(), LengthError> {
        for member in source.members(self.mark) {
            let (key, value) = match member {
                Member::Undefined { end } => {
                    target.write_undefined(end);
                    continue;
                }
                Member::Element(index, value) => (Key::Index(index), value),
                Member::Named(name, value) => (target.key(name), value),
            };
            self.merge_member(target, key, value)?;
        }
        Ok(())
    }

    /// Merges `value`, the member under `key` of a source array or object, into `target`.
    fn merge_member(&self, target: &mut Target<'_>, key: Key<'_>, value: &Value) -> Result<(), LengthError> {
        let rules = self.rules;
        let mark = self.mark;
        if key == Key::Name("__proto__") && container(value).is_some() {
            return Ok(());
        }
        let Some(child) = container(value).filter(|_| !key.listed(&rules.replaced)) else {
            return target.write(key, value.clone(), mark);
        };
        let copied = match (target.get(key), child) {
            (Some(Value::Object(held)), Source::Array(_)) => copy_array_like(held, mark)?,
            _ => None,
        };
        if let Some(mut copied) = copied {
            self.merge_members(&mut Target::Array(JsArray::new(&mut copied, mark)), child)?;
            return target.write(key, Value::Array(copied), mark);
        }
        let onto_fresh = match (target.get_mut(key), child) {
            (Some(Value::Array(held)), Source::Array(items)) => {
                *held = if key.listed(&rules.replaced_arrays) {
                    items.to_vec()
                } else {
                    union(held, items, mark)
                };
                false
            }
            (Some(Value::Object(held)), Source::Object(entries)) if key.listed(&rules.shallow_objects) => {
                for (name, item) in entries {
                    held.insert(name.clone(), item.clone());
                }
                false
            }
            (Some(Value::Object(held)), Source::Object(_)) => {
                self.merge_members(&mut Target::Object(held), child)?;
                false
            }
            (Some(Value::Array(held)), Source::Object(_)) => {
                self.merge_members(&mut Target::Array(JsArray::new(held, mark)), child)?;
                false
            }
            _ => true,
        };
        if onto_fresh {
            target.write(key, fresh_copy(value, &rules.replaced, mark), mark)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "settings_merge_tests.rs"]
mod tests;
