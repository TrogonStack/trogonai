// In-place mutation avoids cloning large tool payloads on the gateway hot path.
// Drop actions must remove keys entirely: downstream audit and anomaly code cannot
// distinguish an explicit JSON null from a field that was never present.

use sha2::{Digest, Sha256};

use super::outcome::{RedactionOutcome, RewriteEntry};
use super::path::{ParsedPath, PathSegment};
use super::rule::RedactionAction;
use super::ruleset::RedactionRuleset;

pub struct RedactionOptions<'a> {
    pub hash_salt: Option<&'a str>,
}

impl Default for RedactionOptions<'_> {
    fn default() -> Self {
        Self { hash_salt: None }
    }
}

#[must_use]
pub fn redact(doc: &mut serde_json::Value, rules: &RedactionRuleset) -> RedactionOutcome {
    redact_with_options(doc, rules, &RedactionOptions::default())
}

#[must_use]
pub fn redact_with_options(
    doc: &mut serde_json::Value,
    rules: &RedactionRuleset,
    options: &RedactionOptions<'_>,
) -> RedactionOutcome {
    let mut outcome = RedactionOutcome::empty();
    for rule in rules.rules() {
        let parsed = match ParsedPath::parse(rule.path.as_str()) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if apply_rule(doc, &parsed.segments, 0, &rule.action, options) {
            outcome
                .rewrites
                .push(RewriteEntry::new(rule.path.as_str(), rule.action.audit_op()));
        }
    }
    outcome
}

fn apply_rule(
    doc: &mut serde_json::Value,
    segments: &[PathSegment],
    idx: usize,
    action: &RedactionAction,
    options: &RedactionOptions<'_>,
) -> bool {
    if idx >= segments.len() {
        return false;
    }
    match &segments[idx] {
        PathSegment::Root => apply_rule(doc, segments, idx + 1, action, options),
        PathSegment::Field(key) => {
            if idx + 1 == segments.len() {
                return mutate_field(doc, key, action, options);
            }
            let Some(child) = doc.as_object_mut().and_then(|obj| obj.get_mut(key)) else {
                return false;
            };
            apply_rule(child, segments, idx + 1, action, options)
        }
        PathSegment::Wildcard => {
            let Some(arr) = doc.as_array_mut() else {
                return false;
            };
            if idx + 1 == segments.len() {
                return drop_all_array_elements(arr);
            }
            let mut any = false;
            let mut i = 0;
            while i < arr.len() {
                if apply_rule(&mut arr[i], segments, idx + 1, action, options) {
                    any = true;
                }
                i += 1;
            }
            any
        }
    }
}

fn mutate_field(
    doc: &mut serde_json::Value,
    key: &str,
    action: &RedactionAction,
    options: &RedactionOptions<'_>,
) -> bool {
    let Some(obj) = doc.as_object_mut() else {
        return false;
    };
    let Some(value) = obj.get_mut(key) else {
        return false;
    };
    match action {
        RedactionAction::Drop => {
            obj.remove(key);
            true
        }
        RedactionAction::Mask => {
            *value = serde_json::Value::String("***".into());
            true
        }
        RedactionAction::Replace(literal) => {
            *value = serde_json::Value::String(literal.clone());
            true
        }
        RedactionAction::Hash => {
            if let Some(digest) = hash_scalar(value, options.hash_salt) {
                *value = serde_json::Value::String(digest);
                true
            } else {
                false
            }
        }
        RedactionAction::RegexReplace { pattern, replacement } => {
            let Some(text) = value.as_str() else {
                return false;
            };
            let Some(replaced) = regex_replace_text(text, pattern, replacement) else {
                return false;
            };
            if replaced == text {
                return false;
            }
            *value = serde_json::Value::String(replaced);
            true
        }
    }
}

fn drop_all_array_elements(arr: &mut Vec<serde_json::Value>) -> bool {
    if arr.is_empty() {
        return false;
    }
    arr.clear();
    true
}

fn hash_scalar(value: &serde_json::Value, salt: Option<&str>) -> Option<String> {
    let canonical = match value {
        serde_json::Value::String(s) => s.as_str(),
        serde_json::Value::Number(n) => return Some(format!("sha256:{}", hash_bytes(n.to_string().as_bytes(), salt))),
        serde_json::Value::Bool(b) => return Some(format!("sha256:{}", hash_bytes(&[u8::from(*b)], salt))),
        _ => return None,
    };
    Some(format!("sha256:{}", hash_bytes(canonical.as_bytes(), salt)))
}

fn hash_bytes(data: &[u8], salt: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    if let Some(s) = salt {
        hasher.update(s.as_bytes());
    }
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn regex_replace_text(text: &str, pattern: &str, replacement: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }
    if is_literal_pattern(pattern) {
        return Some(text.replace(pattern, replacement));
    }
    simple_wildcard_replace(text, pattern, replacement)
}

fn is_literal_pattern(pattern: &str) -> bool {
    !pattern
        .chars()
        .any(|c| matches!(c, '.' | '*' | '+' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '^' | '$' | '|' | '\\'))
}

/// Minimal wildcard matcher: `.` matches any char, `.*` matches any suffix.
fn simple_wildcard_replace(text: &str, pattern: &str, replacement: &str) -> Option<String> {
    if pattern.ends_with(".*") {
        let prefix = &pattern[..pattern.len() - 2];
        if prefix.is_empty() {
            return Some(replacement.to_string());
        }
        if let Some(idx) = text.find(prefix) {
            let mut out = String::new();
            out.push_str(&text[..idx]);
            out.push_str(replacement);
            return Some(out);
        }
        return Some(text.to_string());
    }
    if pattern.contains('.') {
        return regex_dot_replace(text, pattern, replacement);
    }
    Some(text.replace(pattern, replacement))
}

fn regex_dot_replace(text: &str, pattern: &str, replacement: &str) -> Option<String> {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    if let Some(end) = match_dot_pattern(&pat_chars, &text_chars, 0, 0) {
        let mut out = String::new();
        out.extend(&text_chars[..end]);
        out.push_str(replacement);
        out.extend(&text_chars[end..]);
        return Some(out);
    }
    Some(text.to_string())
}

fn match_dot_pattern(pat: &[char], text: &[char], pi: usize, ti: usize) -> Option<usize> {
    if pi == pat.len() {
        return Some(ti);
    }
    if pat[pi] == '.' {
        if pi + 1 == pat.len() {
            return Some(text.len());
        }
        for end in ti..=text.len() {
            if match_dot_pattern(pat, text, pi + 1, end).is_some() {
                return Some(end);
            }
        }
        return None;
    }
    if ti >= text.len() || pat[pi] != text[ti] {
        return None;
    }
    match_dot_pattern(pat, text, pi + 1, ti + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redaction::rule::{JsonPath, RedactionAction, RedactionRule};

    fn mask_rule(path: &str) -> RedactionRule {
        RedactionRule {
            path: JsonPath::parse(path).expect("path"),
            action: RedactionAction::Mask,
        }
    }

    fn hash_rule(path: &str) -> RedactionRule {
        RedactionRule {
            path: JsonPath::parse(path).expect("path"),
            action: RedactionAction::Hash,
        }
    }

    fn drop_rule(path: &str) -> RedactionRule {
        RedactionRule {
            path: JsonPath::parse(path).expect("path"),
            action: RedactionAction::Drop,
        }
    }

    #[test]
    fn redact_with_empty_ruleset_leaves_doc_unchanged() {
        let mut doc = serde_json::json!({ "foo": "bar", "nested": { "secret": 42 } });
        let expected = doc.clone();
        let rules = RedactionRuleset::builder().build();

        let outcome = redact(&mut doc, &rules);

        assert_eq!(doc, expected);
        assert!(outcome.rewrites.is_empty());
    }

    #[test]
    fn mask_replaces_scalar_with_stars() {
        let mut doc = serde_json::json!({ "params": { "api_key": "sk-live" } });
        let rules = RedactionRuleset::builder()
            .rule(mask_rule("$.params.api_key"))
            .build();

        let outcome = redact(&mut doc, &rules);

        assert_eq!(doc["params"]["api_key"], "***");
        assert_eq!(
            outcome.rewrites,
            vec![RewriteEntry::new("$.params.api_key", "mask")]
        );
    }

    #[test]
    fn hash_is_deterministic_with_salt() {
        let mut doc = serde_json::json!({ "params": { "token": "secret" } });
        let rules = RedactionRuleset::builder()
            .rule(hash_rule("$.params.token"))
            .build();
        let options = RedactionOptions {
            hash_salt: Some("tenant-a"),
        };

        let first = redact_with_options(&mut doc, &rules, &options);
        let digest = doc["params"]["token"].as_str().expect("hashed").to_string();

        let mut doc2 = serde_json::json!({ "params": { "token": "secret" } });
        let second = redact_with_options(&mut doc2, &rules, &options);

        assert_eq!(doc2["params"]["token"], digest);
        assert_eq!(first.rewrites, second.rewrites);
        assert!(digest.starts_with("sha256:"));
    }

    #[test]
    fn drop_removes_key_without_null_placeholder() {
        let mut doc = serde_json::json!({ "params": { "internal_note": "x", "keep": 1 } });
        let rules = RedactionRuleset::builder()
            .rule(drop_rule("$.params.internal_note"))
            .build();

        let outcome = redact(&mut doc, &rules);

        assert!(doc["params"].get("internal_note").is_none());
        assert_eq!(doc["params"]["keep"], 1);
        assert_eq!(
            outcome.rewrites,
            vec![RewriteEntry::new("$.params.internal_note", "drop")]
        );
    }

    #[test]
    fn nested_path_mask_targets_deep_field() {
        let mut doc = serde_json::json!({
            "params": { "user": { "email": "alice@acme.com", "name": "Alice" } }
        });
        let rules = RedactionRuleset::builder()
            .rule(mask_rule("$.params.user.email"))
            .build();

        let _ = redact(&mut doc, &rules);

        assert_eq!(doc["params"]["user"]["email"], "***");
        assert_eq!(doc["params"]["user"]["name"], "Alice");
    }

    #[test]
    fn array_wildcard_masks_each_row_ssn() {
        let mut doc = serde_json::json!({
            "result": {
                "rows": [
                    { "ssn": "111", "name": "a" },
                    { "ssn": "222", "name": "b" }
                ]
            }
        });
        let rules = RedactionRuleset::builder()
            .rule(mask_rule("$.result.rows[*].ssn"))
            .build();

        let _ = redact(&mut doc, &rules);

        assert_eq!(doc["result"]["rows"][0]["ssn"], "***");
        assert_eq!(doc["result"]["rows"][1]["ssn"], "***");
    }

    #[test]
    fn rules_apply_in_declaration_order() {
        let mut doc = serde_json::json!({
            "params": { "a": "plain", "b": "secret", "c": "remove" }
        });
        let rules = RedactionRuleset::builder()
            .rule(mask_rule("$.params.a"))
            .rule(hash_rule("$.params.b"))
            .rule(drop_rule("$.params.c"))
            .build();

        let outcome = redact(&mut doc, &rules);

        assert_eq!(doc["params"]["a"], "***");
        assert!(doc["params"]["b"].as_str().unwrap().starts_with("sha256:"));
        assert!(doc["params"].get("c").is_none());
        assert_eq!(outcome.rewrites.len(), 3);
        assert_eq!(outcome.rewrites[0].op, "mask");
        assert_eq!(outcome.rewrites[1].op, "hash");
        assert_eq!(outcome.rewrites[2].op, "drop");
    }

    #[test]
    fn unmatched_optional_field_skips_rewrite_entry() {
        let mut doc = serde_json::json!({ "params": { "token": "x" } });
        let rules = RedactionRuleset::builder()
            .rule(mask_rule("$.params.missing"))
            .rule(hash_rule("$.params.token"))
            .build();

        let outcome = redact(&mut doc, &rules);

        assert_eq!(outcome.rewrites.len(), 1);
        assert_eq!(outcome.rewrites[0].path, "$.params.token");
    }

    #[test]
    fn regex_replace_substitutes_matching_prefix() {
        let mut doc = serde_json::json!({ "params": { "token": "sk-live-abc" } });
        let rules = RedactionRuleset::builder()
            .rule(RedactionRule {
                path: JsonPath::parse("$.params.token").expect("path"),
                action: RedactionAction::RegexReplace {
                    pattern: "sk-live-.*".into(),
                    replacement: "REDACTED".into(),
                },
            })
            .build();

        let outcome = redact(&mut doc, &rules);

        assert_eq!(doc["params"]["token"], "REDACTED");
        assert_eq!(outcome.rewrites[0].op, "regex_replace");
    }
}
