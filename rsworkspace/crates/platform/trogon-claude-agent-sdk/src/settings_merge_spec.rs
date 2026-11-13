//! The rules of the upstream settings merge customizer, taken from the pinned TypeScript bundle.
//!
//! `upstream/settings_schema.json` records them under `merge`; the generator reads each one from the
//! customizer's source, so the drift check covers them and nothing here is transcribed by hand.

use serde_json::Value;

#[derive(Debug, Default)]
pub(crate) struct MergeSpec {
    /// Keys whose source value replaces the destination outright, at any depth.
    pub(crate) replaced: Vec<String>,
    /// Keys whose arrays are replaced rather than unioned.
    pub(crate) replaced_arrays: Vec<String>,
    /// Object keys spread (`{...a, ...b}`) instead of merged deeply.
    pub(crate) shallow_objects: Vec<String>,
}

fn strings(merge: &Value, key: &str) -> Result<Vec<String>, String> {
    let malformed = || format!("merge.{key}");
    merge
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(malformed)?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(malformed))
        .collect()
}

impl MergeSpec {
    pub(crate) fn from_json(merge: &Value) -> Result<Self, String> {
        Ok(Self {
            replaced: strings(merge, "replacedKeys")?,
            replaced_arrays: strings(merge, "replacedArrayKeys")?,
            shallow_objects: strings(merge, "shallowObjectKeys")?,
        })
    }
}
