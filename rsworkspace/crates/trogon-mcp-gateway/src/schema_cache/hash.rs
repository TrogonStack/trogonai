use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::key::SchemaHash;

/// Canonical JSON for schema hashing: object keys sorted lexicographically at each
/// object level; arrays preserve element order; UTF-8 JSON without insignificant whitespace.
pub fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(&canonicalize(value)).unwrap_or_default()
}

pub fn hash_schema(value: &Value) -> SchemaHash {
    SchemaHash::from_bytes(Sha256::digest(canonical_json_bytes(value)).into())
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map
                .iter()
                .map(|(key, child)| (key.as_str(), canonicalize(child)))
                .collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_object_keys() {
        let a = serde_json::json!({"z": 1, "a": 2});
        let b = serde_json::json!({"a": 2, "z": 1});
        assert_eq!(canonical_json_bytes(&a), canonical_json_bytes(&b));
    }

    #[test]
    fn schema_hash_is_stable_across_key_order() {
        let first = serde_json::json!({"type": "object", "properties": {"b": {"type": "string"}, "a": {"type": "integer"}}});
        let second = serde_json::json!({"properties": {"a": {"type": "integer"}, "b": {"type": "string"}}, "type": "object"});
        assert_eq!(hash_schema(&first), hash_schema(&second));
    }

    #[test]
    fn schema_hash_pin_vector() {
        let schema = serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}});
        let hash = hash_schema(&schema);
        assert_eq!(
            hash.as_hex(),
            "2b7196d853bac7cea83330be9c2073848dedc10746eaf403bb5f73687531baf2"
        );
    }
}
