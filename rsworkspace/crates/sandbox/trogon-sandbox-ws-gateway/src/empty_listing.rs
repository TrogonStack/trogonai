use serde_json::{Map, Value};

/// `{ "<key>": [] }`, the answer of a backend that tracks nothing.
pub(crate) fn empty_listing(key: &str) -> Value {
    let mut listing = Map::new();
    listing.insert(key.to_string(), Value::Array(Vec::new()));
    Value::Object(listing)
}
