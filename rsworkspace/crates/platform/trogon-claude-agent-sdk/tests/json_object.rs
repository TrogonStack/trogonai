use trogon_claude_agent_sdk::JsonObject;

#[test]
fn json_objects_retain_nested_values_without_accepting_scalar_roots() -> Result<(), serde_json::Error> {
    let raw = r#"{"null":null,"array":[1,true,"text"],"object":{"extension":7}}"#;
    let object: JsonObject = serde_json::from_str(raw)?;
    assert_eq!(
        serde_json::to_value(object.clone())?,
        serde_json::from_str::<serde_json::Value>(raw)?
    );
    assert_eq!(object.get("null"), Some(&serde_json::Value::Null));
    let mut changed = object.clone();
    changed.remove("object");
    assert!(object.contains_key("object"));
    for scalar in ["null", "[]", "false", "7", "\"text\""] {
        assert!(serde_json::from_str::<JsonObject>(scalar).is_err());
    }
    Ok(())
}
