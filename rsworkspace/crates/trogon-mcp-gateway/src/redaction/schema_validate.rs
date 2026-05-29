use super::path::{ParsedPath, PathSegment};

/// Check whether a JSONPath resolves against a JSON Schema document.
pub fn path_in_schema(schema: &serde_json::Value, path: &ParsedPath) -> bool {
    let mut current = schema;
    let mut idx = 0usize;
    while idx < path.segments.len() {
        match &path.segments[idx] {
            PathSegment::Root => {
                idx += 1;
            }
            PathSegment::Field(name) => {
                let Some(next) = schema_field(current, name) else {
                    return false;
                };
                current = next;
                idx += 1;
            }
            PathSegment::Wildcard => {
                let Some(items) = current.get("items") else {
                    return false;
                };
                current = items;
                idx += 1;
            }
        }
    }
    true
}

fn schema_field<'a>(schema: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    if let Some(props) = schema.get("properties").and_then(|p| p.get(name)) {
        return Some(props);
    }
    if schema.get("additionalProperties").is_some_and(|v| !v.is_boolean() || v.as_bool() == Some(true)) {
        return schema.get("additionalProperties").filter(|v| !v.is_boolean());
    }
    if schema.get("type").and_then(|t| t.as_str()) == Some("object") {
        return schema.get("additionalProperties");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redaction::path::ParsedPath;

    #[test]
    fn nested_property_exists_in_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "params": {
                    "type": "object",
                    "properties": {
                        "user": {
                            "type": "object",
                            "properties": {
                                "email": { "type": "string" }
                            }
                        }
                    }
                }
            }
        });
        let path = ParsedPath::parse("$.params.user.email").expect("path");
        assert!(path_in_schema(&schema, &path));
    }

    #[test]
    fn array_wildcard_requires_items_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "result": {
                    "type": "object",
                    "properties": {
                        "rows": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "ssn": { "type": "string" }
                                }
                            }
                        }
                    }
                }
            }
        });
        let path = ParsedPath::parse("$.result.rows[*].ssn").expect("path");
        assert!(path_in_schema(&schema, &path));
    }

    #[test]
    fn unknown_field_not_in_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "params": {
                    "type": "object",
                    "properties": {
                        "token": { "type": "string" }
                    }
                }
            }
        });
        let path = ParsedPath::parse("$.params.secret").expect("path");
        assert!(!path_in_schema(&schema, &path));
    }
}
