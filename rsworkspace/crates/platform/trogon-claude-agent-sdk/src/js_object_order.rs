//! JavaScript own-key enumeration order for JSON objects, shared by the TypeScript session helpers.

use serde_json::Value;

use crate::JsonObject;

/// The array index a property name spells: its canonical decimal form below 2^32 - 1.
pub(crate) fn array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || (!key.starts_with('0') && key.bytes().all(|byte| byte.is_ascii_digit()));
    if !canonical || key.is_empty() || key.len() > 10 {
        return None;
    }
    key.parse::<u32>().ok().filter(|number| *number != u32::MAX)
}

/// The own keys of a JavaScript object in enumeration order: array indices ascending first, then the other keys in
/// insertion order.
pub(crate) fn js_key_order(map: &JsonObject) -> Vec<(&String, &Value)> {
    let mut indexed: Vec<(u32, &String, &Value)> = Vec::new();
    let mut named: Vec<(&String, &Value)> = Vec::new();
    for (key, value) in map {
        match array_index(key) {
            Some(number) => indexed.push((number, key, value)),
            None => named.push((key, value)),
        }
    }
    indexed.sort_by_key(|(number, _, _)| *number);
    indexed
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .chain(named)
        .collect()
}

/// The own keys of a JavaScript object in enumeration order, `Object.keys(object)`.
pub(crate) fn js_keys(map: &JsonObject) -> Vec<&String> {
    js_key_order(map).into_iter().map(|(key, _)| key).collect()
}

/// Moves the array-index keys of `object` first, in ascending order, keeping the other keys in insertion order.
/// Only this object is reordered; its values are left as they are.
pub(crate) fn js_reorder(object: &mut JsonObject) {
    if object.keys().any(|key| array_index(key).is_some()) {
        let (mut indexed, named): (Vec<_>, Vec<_>) = std::mem::take(object)
            .into_iter()
            .partition(|(key, _)| array_index(key).is_some());
        indexed.sort_by_key(|(key, _)| array_index(key));
        object.extend(indexed.into_iter().chain(named));
    }
}

/// The value as a JavaScript object graph holds it: every object's keys in enumeration order.
pub(crate) fn js_ordered(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(js_ordered).collect()),
        Value::Object(map) => Value::Object(js_ordered_object(map)),
        other => other,
    }
}

/// [`js_ordered`] of an object, kept as the map it already is.
pub(crate) fn js_ordered_object(object: JsonObject) -> JsonObject {
    let mut object: JsonObject = object.into_iter().map(|(key, item)| (key, js_ordered(item))).collect();
    js_reorder(&mut object);
    object
}
