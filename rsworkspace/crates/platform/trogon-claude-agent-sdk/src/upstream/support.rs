//! Serde helpers shared by the generated upstream types.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Distinguishes an omitted field (`None`) from an explicit `null` (`Some(None)`).
pub(super) mod double_option {
    use super::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

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

/// Keeps the candidate with the fewest unknown keys; the earliest declared wins ties.
pub(super) fn keep_best<T>(best: &mut Option<(usize, T)>, candidate: Option<(usize, T)>) {
    if let Some((score, value)) = candidate
        && best.as_ref().is_none_or(|(current, _)| score < *current)
    {
        *best = Some((score, value));
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Probe {
        #[serde(default, skip_serializing_if = "Option::is_none", with = "super::double_option")]
        tri: Option<Option<String>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "super::optional_value"
        )]
        free: Option<Value>,
    }

    #[test]
    fn omitted_null_and_present_stay_distinct() -> Result<(), serde_json::Error> {
        for fixture in [
            json!({}),
            json!({"tri": null, "free": null}),
            json!({"tri": "x", "free": {"a": 1}}),
        ] {
            let probe: Probe = serde_json::from_value(fixture.clone())?;
            assert_eq!(serde_json::to_value(&probe)?, fixture);
        }
        let null: Probe = serde_json::from_value(json!({"tri": null}))?;
        assert_eq!(null.tri, Some(None));
        let omitted: Probe = serde_json::from_value(json!({}))?;
        assert_eq!(omitted.tri, None);
        Ok(())
    }
}
