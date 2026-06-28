use super::*;

#[test]
fn json_pointer_from_dollar_path() {
    let root = serde_json::json!({"params": {"secret": "x"}});
    assert_eq!(
        value_at_path(&root, "$.params.secret"),
        Some(&serde_json::Value::String("x".into()))
    );
}

#[test]
fn bracket_index_in_dollar_path() {
    let root = serde_json::json!({"params": {"parts": [{"text": "x"}]}});
    assert_eq!(
        value_at_path(&root, "$.params.parts[0].text"),
        Some(&serde_json::Value::String("x".into()))
    );
}

#[test]
fn slash_pointer_resolves() {
    let root = serde_json::json!({"params": {"n": 1}});
    assert_eq!(
        value_at_path(&root, "/params/n"),
        Some(&serde_json::Value::Number(1.into()))
    );
}

#[test]
fn dollar_root_pointer_is_empty_string() {
    let root = serde_json::json!({"x": 1});
    assert_eq!(value_at_path(&root, "$."), Some(&root));
}

#[test]
fn unprefixed_path_returns_none() {
    let root = serde_json::json!({"x": 1});
    assert!(value_at_path(&root, "params.x").is_none());
}
