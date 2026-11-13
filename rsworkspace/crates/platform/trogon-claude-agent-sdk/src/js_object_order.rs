//! JavaScript own-key enumeration order for JSON objects, shared by the TypeScript session helpers.

use serde_json::Value;

use crate::JsonObject;

/// The own keys of a JavaScript object in enumeration order: array indices ascending first, then the other keys in
/// insertion order.
pub(crate) fn js_key_order(map: &JsonObject) -> Vec<(&String, &Value)> {
    fn index(key: &str) -> Option<u32> {
        let canonical = key == "0" || (!key.starts_with('0') && key.bytes().all(|byte| byte.is_ascii_digit()));
        if !canonical || key.is_empty() || key.len() > 10 {
            return None;
        }
        key.parse::<u32>().ok().filter(|number| *number != u32::MAX)
    }
    let mut indexed: Vec<(u32, &String, &Value)> = Vec::new();
    let mut named: Vec<(&String, &Value)> = Vec::new();
    for (key, value) in map {
        match index(key) {
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

/// The value as a JavaScript object graph holds it: every object's keys in enumeration order.
pub(crate) fn js_ordered(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(js_ordered).collect()),
        Value::Object(map) => {
            let order: Vec<String> = js_key_order(&map).into_iter().map(|(key, _)| key.clone()).collect();
            let mut map = map;
            Value::Object(
                order
                    .into_iter()
                    .filter_map(|key| map.remove(&key).map(|item| (key, js_ordered(item))))
                    .collect(),
            )
        }
        other => other,
    }
}
