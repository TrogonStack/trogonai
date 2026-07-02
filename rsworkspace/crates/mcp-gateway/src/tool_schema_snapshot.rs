use crate::{Sha256Digest, ToolName};
use serde_json::Value;
use std::collections::BTreeMap;

/// A single MCP tool's schema as recorded in a drift baseline or current
/// snapshot: name, description, per-parameter JSON schemas, and the list of
/// required parameter names.
///
/// This is intentionally a separate type from [`crate::ToolInputSchema`]:
/// that type wraps a whole `tools/list` input schema JSON object for the
/// poisoning/rug-pull/hidden-instruction detectors, while this type models
/// AGT's flattened `ToolSchema` dataclass (`name`, `description`,
/// `parameters: dict[str, Any]`, `required: list[str]`) used specifically
/// for schema-drift comparison and fingerprinting.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSchemaSnapshot {
    name: ToolName,
    description: String,
    parameters: BTreeMap<String, Value>,
    required: Vec<String>,
}

impl ToolSchemaSnapshot {
    pub fn new(
        name: ToolName,
        description: impl Into<String>,
        parameters: BTreeMap<String, Value>,
        required: Vec<String>,
    ) -> Self {
        Self {
            name,
            description: description.into(),
            parameters,
            required,
        }
    }

    pub fn name(&self) -> &ToolName {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn parameters(&self) -> &BTreeMap<String, Value> {
        &self.parameters
    }

    pub fn required(&self) -> &[String] {
        &self.required
    }

    pub fn is_required(&self, parameter_name: &str) -> bool {
        self.required.iter().any(|r| r == parameter_name)
    }

    /// Content hash for change detection, truncated to 16 hex characters to
    /// match AGT's `ToolSchema.fingerprint()`
    /// (`hashlib.sha256(...).hexdigest()[:16]`).
    pub fn fingerprint(&self) -> String {
        let mut sorted_required = self.required.clone();
        sorted_required.sort();

        // `BTreeMap` + `serde_json::Map`'s `BTreeMap` backing both serialize
        // with sorted keys, matching AGT's `json.dumps(..., sort_keys=True)`.
        let canonical = serde_json::json!({
            "name": self.name.as_str(),
            "description": self.description,
            "parameters": self.parameters,
            "required": sorted_required,
        });
        // Serializing a `serde_json::Value` built from owned data never fails.
        #[allow(clippy::unwrap_used)]
        let content = serde_json::to_string(&canonical).unwrap_or_default();
        let digest = Sha256Digest::of(content.as_bytes());
        digest.as_str().chars().take(16).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(description: &str, params: &[(&str, Value)], required: &[&str]) -> ToolSchemaSnapshot {
        ToolSchemaSnapshot::new(
            ToolName::new("search").expect("valid"),
            description,
            params.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect(),
            required.iter().map(|s| (*s).to_string()).collect(),
        )
    }

    #[test]
    fn fingerprint_is_16_hex_chars() {
        let s = schema("Search the web", &[], &[]);
        let fp = s.fingerprint();
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let a = schema("Search the web", &[("q", json!({"type": "string"}))], &["q"]);
        let b = schema("Search the web", &[("q", json!({"type": "string"}))], &["q"]);
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_ignores_required_order() {
        let a = schema("d", &[], &["a", "b"]);
        let b = schema("d", &[], &["b", "a"]);
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_changes_when_description_changes() {
        let a = schema("Search the web", &[], &[]);
        let b = schema("Steal all data", &[], &[]);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn is_required_checks_membership() {
        let s = schema("d", &[], &["q"]);
        assert!(s.is_required("q"));
        assert!(!s.is_required("other"));
    }
}
