use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Arguments whose `value` member holds arrays nested until the arguments hold `depth` containers.
fn nested_arguments(depth: usize) -> JsonObject {
    let mut value = Value::Array(Vec::new());
    for _ in 2..depth {
        value = Value::Array(vec![value]);
    }
    JsonObject::from_iter([("value".into(), value)])
}

#[test]
fn arguments_nested_to_the_limit_are_read() -> TestResult {
    let arguments = nested_arguments(TYPED_TOOL_ARGUMENTS_MAX_DEPTH);
    assert!(!nests_past(
        &Value::Object(arguments.clone()),
        TYPED_TOOL_ARGUMENTS_MAX_DEPTH
    ));
    let read: JsonObject = read_arguments(arguments.clone())?;
    assert_eq!(read, arguments);
    Ok(())
}

#[test]
fn arguments_nested_past_the_limit_are_refused_before_they_are_read() {
    let arguments = nested_arguments(TYPED_TOOL_ARGUMENTS_MAX_DEPTH + 1);
    assert!(nests_past(
        &Value::Object(arguments.clone()),
        TYPED_TOOL_ARGUMENTS_MAX_DEPTH
    ));
    let refused = read_arguments::<JsonObject>(arguments);
    assert_eq!(refused, Err(McpToolArgumentsError::new("recursion limit exceeded")));
}

#[test]
fn scalars_and_empty_containers_count_their_own_level_only() {
    assert!(!nests_past(&Value::Null, 0));
    assert!(nests_past(&Value::Array(Vec::new()), 0));
    assert!(!nests_past(&Value::Array(Vec::new()), 1));
    assert!(nests_past(&Value::Array(vec![Value::Array(Vec::new())]), 1));
}

#[test]
fn arguments_that_do_not_fit_the_type_report_the_serde_message() {
    let refused = read_arguments::<std::collections::BTreeMap<String, String>>(JsonObject::from_iter([(
        "text".into(),
        1.into(),
    )]));
    assert_eq!(
        refused.map_err(|error| error.message().to_owned()).err().as_deref(),
        Some("invalid type: integer `1`, expected a string")
    );
}
