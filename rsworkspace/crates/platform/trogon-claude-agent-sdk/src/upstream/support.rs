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

/// The serializer an object falls back to when it does not encode as a `serde_json::Value`: its JSON text,
/// written into a buffer.
pub(super) type TextSerializer<'a> = serde_json::Serializer<&'a mut Vec<u8>>;

/// Writes the members an object keeps under undeclared keys: each nested past `SHALLOW` levels as its compact JSON
/// text, so writing the object walks no deeper than that.
pub(super) fn shallow_members<S: Serializer>(
    members: &indexmap::IndexMap<String, crate::DeepJsonValue>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_map(
        members
            .iter()
            .map(|(key, member)| (key, crate::deep_json::ShallowWrite(member.as_value()))),
    )
}

/// Writes an encoded object with its keys in the recorded order. An object holding a member under an undeclared key
/// nested past what `serde_json` reads back does not encode as a `serde_json::Value`, so `text` writes it as JSON
/// text instead, and its members are written in the recorded order as that text holds them.
pub(super) fn ordered<S: Serializer>(
    encoded: Result<serde_json::Value, serde_json::Error>,
    text: impl FnOnce(&mut TextSerializer<'_>) -> Result<(), serde_json::Error>,
    order: &KeyOrder,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let Ok(encoded) = encoded else {
        return ordered_text(text, order, serializer);
    };
    match encoded {
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

/// [`ordered`] for an object `text` writes as JSON text: each member is copied as the text holds it.
fn ordered_text<S: Serializer>(
    text: impl FnOnce(&mut TextSerializer<'_>) -> Result<(), serde_json::Error>,
    order: &KeyOrder,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut bytes = Vec::new();
    text(&mut serde_json::Serializer::new(&mut bytes)).map_err(serde::ser::Error::custom)?;
    let mut members: indexmap::IndexMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(&bytes).map_err(serde::ser::Error::custom)?;
    let mut out = indexmap::IndexMap::with_capacity(members.len());
    for key in &order.0 {
        if let Some(member) = members.swap_remove(key) {
            out.insert(key.clone(), member);
        }
    }
    out.extend(members);
    out.serialize(serializer)
}

/// Decodes `value` as `T` only when encoding it again reproduces `value` exactly.
pub(super) fn exact<T: Serialize + serde::de::DeserializeOwned>(value: &serde_json::Value) -> Option<T> {
    let inner: T = serde_json::from_value(value.clone()).ok()?;
    crate::deep_json::encode(&inner)
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

/// Whether `value` holds one of the listed strings under `key`.
pub(super) fn tag_str(value: &serde_json::Value, key: &str, allowed: &[&str]) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|text| allowed.contains(&text))
}

/// Whether `value` holds one of the listed booleans under `key`.
pub(super) fn tag_bool(value: &serde_json::Value, key: &str, allowed: &[bool]) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .is_some_and(|flag| allowed.contains(&flag))
}

/// Keeps the candidate that pins the most tags, then the one with the fewest unknown keys; the
/// earliest declared wins ties.
pub(super) fn keep_most_specific<T>(
    best: &mut Option<(usize, usize, T)>,
    specificity: usize,
    candidate: Option<(usize, T)>,
) {
    if let Some((score, value)) = candidate
        && best
            .as_ref()
            .is_none_or(|(pinned, current, _)| specificity > *pinned || (specificity == *pinned && score < *current))
    {
        *best = Some((specificity, score, value));
    }
}

#[cfg(test)]
mod tests;
