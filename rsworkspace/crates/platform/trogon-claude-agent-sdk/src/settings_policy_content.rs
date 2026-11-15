//! Whether a settings document carries policy beyond its meta keys.
//!
//! Both the schema parse and the resolver ask this, so it sits below either of them.

use serde_json::{Map, Value};

use crate::constants::POLICY_META_KEYS;

/// Whether a settings document holds any key beyond the policy meta keys (`zu` upstream).
pub(crate) fn has_policy_content(settings: &Map<String, Value>) -> bool {
    settings.keys().any(|key| !POLICY_META_KEYS.contains(&key.as_str()))
}

pub(crate) fn is_empty_tree(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.values().all(is_empty_tree),
        _ => false,
    }
}

/// Whether a document holds a value that is not an empty object tree (`Bu` upstream).
pub(crate) fn has_authored_value(settings: &Map<String, Value>) -> bool {
    settings
        .iter()
        .any(|(key, value)| !POLICY_META_KEYS.contains(&key.as_str()) && !is_empty_tree(value))
}
