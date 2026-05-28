// In-place mutation avoids cloning large tool payloads on the gateway hot path.
// Drop actions must remove keys entirely: downstream audit and anomaly code cannot
// distinguish an explicit JSON null from a field that was never present.

use super::ruleset::RedactionRuleset;

pub fn redact(doc: &mut serde_json::Value, rules: &RedactionRuleset) {
    let _ = (doc, rules);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redaction::RedactionRuleset;

    #[test]
    fn redact_with_empty_ruleset_leaves_doc_unchanged() {
        let mut doc = serde_json::json!({ "foo": "bar", "nested": { "secret": 42 } });
        let expected = doc.clone();
        let rules = RedactionRuleset::builder().build();

        redact(&mut doc, &rules);

        assert_eq!(doc, expected);
    }
}
