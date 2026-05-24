use serde::Deserialize;
use serde_json::Value;

const MASK: &str = "[REDACTED]";

const DEFAULT_DENY_PATHS: &[&str] = &[
    "$.metadata.credentials",
    "$.metadata.internal",
    "$.metadata.secret",
    "$.extensions.secret",
];

const DEFAULT_ALLOW_PATHS: &[&str] = &["$.content", "$.text", "$.message_id"];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SanitizerConfig {
    #[serde(default = "default_deny_paths")]
    pub deny_paths: Vec<String>,
    #[serde(default = "default_allow_paths")]
    pub allow_paths: Vec<String>,
}

impl Default for SanitizerConfig {
    fn default() -> Self {
        Self {
            deny_paths: default_deny_paths(),
            allow_paths: default_allow_paths(),
        }
    }
}

fn default_deny_paths() -> Vec<String> {
    DEFAULT_DENY_PATHS.iter().map(|s| (*s).to_owned()).collect()
}

fn default_allow_paths() -> Vec<String> {
    DEFAULT_ALLOW_PATHS.iter().map(|s| (*s).to_owned()).collect()
}

pub fn redact_json_bytes(input: &[u8]) -> Result<Vec<u8>, RedactError> {
    let mut value: Value = serde_json::from_slice(input).map_err(RedactError::Json)?;
    let config = extract_config(&value).unwrap_or_default();
    strip_config(&mut value);
    sanitize_value(&mut value, &[], &config);
    serde_json::to_vec(&value).map_err(RedactError::Json)
}

fn extract_config(value: &Value) -> Option<SanitizerConfig> {
    value
        .get("__sanitizer_config__")
        .and_then(|raw| serde_json::from_value(raw.clone()).ok())
}

fn strip_config(value: &mut Value) {
    if let Value::Object(map) = value {
        map.remove("__sanitizer_config__");
    }
}

fn sanitize_value(value: &mut Value, path: &[String], config: &SanitizerConfig) {
    match value {
        Value::String(text) => {
            if should_redact(path, config) {
                *text = MASK.to_owned();
            }
        }
        Value::Array(items) => {
            for (idx, item) in items.iter_mut().enumerate() {
                let mut next = path.to_owned();
                next.push(idx.to_string());
                sanitize_value(item, &next, config);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                let mut next = path.to_owned();
                next.push(key.clone());
                sanitize_value(item, &next, config);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn should_redact(path: &[String], config: &SanitizerConfig) -> bool {
    if path.is_empty() {
        return false;
    }
    let dotted = format!("$.{}", path.join("."));
    let deny_specificity = config
        .deny_paths
        .iter()
        .filter(|rule| path_matches(rule, &dotted))
        .map(|rule| normalize_segments(rule).len())
        .max();
    let allow_specificity = config
        .allow_paths
        .iter()
        .filter(|rule| path_matches(rule, &dotted))
        .map(|rule| normalize_segments(rule).len())
        .max();
    match (deny_specificity, allow_specificity) {
        (Some(deny), Some(allow)) if allow >= deny => false,
        (Some(_), _) => true,
        _ => false,
    }
}

fn path_matches(rule: &str, path: &str) -> bool {
    let rule_segments = normalize_segments(rule);
    let path_segments = normalize_segments(path);
    if rule_segments.is_empty() {
        return false;
    }
    'outer: for start in 0..path_segments.len().saturating_sub(rule_segments.len()) + 1 {
        for (idx, rule_seg) in rule_segments.iter().enumerate() {
            let Some(path_seg) = path_segments.get(start + idx) else {
                continue 'outer;
            };
            if !segments_match(rule_seg, path_seg) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

fn segments_match(rule_seg: &str, path_seg: &str) -> bool {
    rule_seg == "*" || rule_seg == path_seg
}

fn normalize_segments(path: &str) -> Vec<String> {
    let trimmed = path
        .strip_prefix("$")
        .unwrap_or(path)
        .trim_start_matches('.');
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split('.')
        .flat_map(expand_segment)
        .collect()
}

fn expand_segment(segment: &str) -> Vec<String> {
    if let Some(base) = segment.strip_suffix("[*]") {
        if base.is_empty() {
            vec!["*".to_owned()]
        } else {
            vec![base.to_owned(), "*".to_owned()]
        }
    } else if let Some(base) = segment.split('[').next() {
        vec![base.to_owned()]
    } else {
        vec![segment.to_owned()]
    }
}

#[derive(Debug)]
pub enum RedactError {
    Json(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylisted_metadata_paths_are_redacted() {
        let input = br#"{"metadata":{"credentials":"secret-token","note":"visible"},"text":"hello"}"#;
        let got = redact_json_bytes(input).expect("json");
        let value: Value = serde_json::from_slice(&got).expect("parse");
        assert_eq!(value["metadata"]["credentials"].as_str(), Some(MASK));
        assert_eq!(value["metadata"]["note"].as_str(), Some("visible"));
        assert_eq!(value["text"].as_str(), Some("hello"));
    }

    #[test]
    fn runtime_config_overrides_defaults() {
        let input = br#"{
            "__sanitizer_config__": {
                "deny_paths": ["$.payload.token"],
                "allow_paths": ["$.payload"]
            },
            "payload": {"token": "abc", "label": "ok"}
        }"#;
        let got = redact_json_bytes(input).expect("json");
        let value: Value = serde_json::from_slice(&got).expect("parse");
        assert!(value.get("__sanitizer_config__").is_none());
        assert_eq!(value["payload"]["token"].as_str(), Some(MASK));
        assert_eq!(value["payload"]["label"].as_str(), Some("ok"));
    }

    #[test]
    fn path_matcher_supports_wildcards() {
        assert!(path_matches("$.metadata[*].secret", "$.metadata.0.secret"));
        assert!(!path_matches("$.metadata.note", "$.metadata.credentials"));
    }
}
