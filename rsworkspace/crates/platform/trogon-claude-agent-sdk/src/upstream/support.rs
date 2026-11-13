//! Serde helpers shared by the generated upstream types.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(super) mod double_option;

/// Keeps an explicit `null` in an optional `unknown` field as `Some(Value::Null)`.
pub(super) fn optional_value<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// The key order an object had when it was decoded, so encoding it again is byte-identical.
///
/// It is metadata, not data: it never affects equality. Keys missing from it, such as fields
/// set after decoding, are written after the recorded ones in declaration order.
#[derive(Clone, Debug, Default)]
pub struct KeyOrder(Vec<String>);

impl KeyOrder {
    pub fn of(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Object(map) => Self(map.keys().cloned().collect()),
            _ => Self::default(),
        }
    }
}

impl PartialEq for KeyOrder {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for KeyOrder {}

/// Writes an encoded object with its keys in the recorded order.
pub(super) fn ordered<S: Serializer>(
    encoded: Result<serde_json::Value, serde_json::Error>,
    order: &KeyOrder,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match encoded.map_err(serde::ser::Error::custom)? {
        serde_json::Value::Object(mut map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for key in &order.0 {
                if let Some(value) = map.remove(key) {
                    out.insert(key.clone(), value);
                }
            }
            out.extend(map);
            out.serialize(serializer)
        }
        other => other.serialize(serializer),
    }
}

/// Decodes `value` as `T` only when encoding it again reproduces `value` exactly.
pub(super) fn exact<T: Serialize + serde::de::DeserializeOwned>(value: &serde_json::Value) -> Option<T> {
    let inner: T = serde_json::from_value(value.clone()).ok()?;
    serde_json::to_value(&inner)
        .is_ok_and(|round| round == *value)
        .then_some(inner)
}

/// The number of top-level keys of `value` that a struct does not declare.
pub(super) fn unknown_keys(value: &serde_json::Value, known: &[&str]) -> usize {
    value
        .as_object()
        .map_or(0, |map| map.keys().filter(|key| !known.contains(&key.as_str())).count())
}

/// The number of the listed string-literal properties of `value` that hold a string the
/// declarations do not list.
pub(super) fn unlisted_literals(value: &serde_json::Value, literals: &[(&str, &[&str])]) -> usize {
    let Some(map) = value.as_object() else {
        return 0;
    };
    literals
        .iter()
        .filter(|(key, allowed)| {
            map.get(*key)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !allowed.contains(&text))
        })
        .count()
}

/// Keeps the candidate with the fewest unknown keys; the earliest declared wins ties.
pub(super) fn keep_best<T>(best: &mut Option<(usize, T)>, candidate: Option<(usize, T)>) {
    if let Some((score, value)) = candidate
        && best.as_ref().is_none_or(|(current, _)| score < *current)
    {
        *best = Some((score, value));
    }
}

#[cfg(test)]
mod tests;
