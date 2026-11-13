#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact JSON Schema shapes, including internal keywords."
    )
)]

use super::*;
use crate::sdk_mcp_zod::Node;
use crate::sdk_mcp_zod_run::Input;
use crate::{SchemaPointer, SchemaPointerError, ValidationChoices, ValidationChoicesError, ValidationChoicesOptions};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn schema(value: Value) -> Result<JsonObject, &'static str> {
    match value {
        Value::Object(object) => Ok(object),
        _ => Err("not an object"),
    }
}

fn pointer(text: &str) -> Result<SchemaPointer, SchemaPointerError> {
    SchemaPointer::parse(text)
}

fn only(choice: SchemaChoice) -> Result<ValidationChoices, Box<dyn std::error::Error>> {
    Ok(ValidationChoices::new(ValidationChoicesOptions {
        choices: vec![(pointer("/properties/x")?, choice)],
        callbacks: Vec::new(),
    })?)
}

fn parse(
    choices: &ValidationChoices,
    schema: &JsonObject,
    input: &Value,
) -> Result<crate::sdk_mcp_zod::Parsed, SchemaPointer> {
    let prepared = choices.prepare(schema)?;
    Ok(Node::from_schema(&prepared.schema, &prepared.callbacks).parse(Input::Json(input)))
}

#[test]
fn pointers_follow_rfc_6901() {
    assert!(pointer("").is_ok());
    assert!(pointer("/properties/x").is_ok());
    assert!(pointer("/a~0b/c~1d").is_ok());
    assert!(pointer("properties").is_err());
    assert!(pointer("/a~2").is_err());
    assert!(pointer("/a~").is_err());
    assert_eq!(pointer("/a").map(|pointer| pointer.to_string()), Ok("/a".to_owned()));
}

#[test]
fn duplicate_choices_callbacks_and_incomplete_orders_are_typed_errors() -> TestResult {
    let at = pointer("/properties/x")?;
    let twice = ValidationChoices::new(ValidationChoicesOptions {
        choices: vec![
            (at.clone(), SchemaChoice::Length(LengthChoice::Exact)),
            (at.clone(), SchemaChoice::Length(LengthChoice::MinMax)),
        ],
        callbacks: Vec::new(),
    });
    assert!(matches!(
        twice,
        Err(ValidationChoicesError::DuplicateChoice { kind: "length", .. })
    ));
    let both = ValidationChoices::new(ValidationChoicesOptions {
        choices: vec![
            (at.clone(), SchemaChoice::Length(LengthChoice::Exact)),
            (at.clone(), SchemaChoice::RegexFlags("u".into())),
        ],
        callbacks: Vec::new(),
    });
    assert!(both.is_ok());
    let callback = SchemaCallback::from(SchemaCallbackOptions::default());
    let callbacks = ValidationChoices::new(ValidationChoicesOptions {
        choices: Vec::new(),
        callbacks: vec![(at.clone(), callback.clone()), (at.clone(), callback)],
    });
    assert!(matches!(
        callbacks,
        Err(ValidationChoicesError::DuplicateCallback { .. })
    ));
    let partial = ValidationChoices::new(ValidationChoicesOptions {
        choices: vec![(
            at,
            SchemaChoice::StringCheckOrder(vec![
                StringCheckKind::Minimum,
                StringCheckKind::Minimum,
                StringCheckKind::Format,
            ]),
        )],
        callbacks: Vec::new(),
    });
    assert!(matches!(
        partial,
        Err(ValidationChoicesError::IncompleteCheckOrder { .. })
    ));
    Ok(())
}

#[test]
fn preparing_writes_the_choices_into_a_copy_and_leaves_the_listed_schema_alone() -> TestResult {
    let listed = schema(json!({"type":"object","properties":{"x":{"type":"string","minLength":3,"maxLength":3}}}))?;
    let before = listed.clone();
    let choices = only(SchemaChoice::Length(LengthChoice::MinMax))?;
    let prepared = choices.prepare(&listed).map_err(|pointer| pointer.to_string())?;
    assert_eq!(listed, before);
    assert_eq!(
        prepared.schema.pointer("/properties/x/x-trogon-min-max"),
        Some(&Value::Bool(true))
    );
    assert!(prepared.callbacks.is_empty());
    Ok(())
}

#[test]
fn a_pointer_outside_the_schema_is_returned() -> TestResult {
    let listed = schema(json!({"type":"object","properties":{"y":{"type":"string"}}}))?;
    let choices = only(SchemaChoice::Length(LengthChoice::MinMax))?;
    assert_eq!(
        choices.prepare(&listed).err().map(|pointer| pointer.to_string()),
        Some("/properties/x".to_owned())
    );
    Ok(())
}

#[test]
fn a_default_choice_leaves_the_schema_as_listed() -> TestResult {
    let listed = schema(json!({"type":"object","properties":{"x":{"type":"string"}}}))?;
    let choices = only(SchemaChoice::Union(UnionChoice::Plain))?;
    let prepared = choices.prepare(&listed).map_err(|pointer| pointer.to_string())?;
    assert_eq!(prepared.schema, Value::Object(listed));
    Ok(())
}

#[test]
fn the_exact_length_choice_reports_one_exact_issue_and_min_max_reports_a_bound() -> TestResult {
    let listed = schema(
        json!({"type":"object","properties":{"x":{"type":"string","minLength":3,"maxLength":3}},"required":["x"]}),
    )?;
    let input = json!({"x":"ab"});
    let exact = parse(&ValidationChoices::default(), &listed, &input).map_err(|pointer| pointer.to_string())?;
    assert_eq!(exact.issues.len(), 1);
    assert_eq!(exact.issues[0].get("exact"), Some(&Value::Bool(true)));
    let split = parse(&only(SchemaChoice::Length(LengthChoice::MinMax))?, &listed, &input)
        .map_err(|pointer| pointer.to_string())?;
    assert_eq!(split.issues.len(), 1);
    assert_eq!(split.issues[0].get("exact"), None);
    Ok(())
}

#[test]
fn the_check_order_choice_orders_the_issues() -> TestResult {
    let listed = schema(
        json!({"type":"object","properties":{"x":{"type":"string","minLength":3,"maxLength":5,"pattern":"^a"}},"required":["x"]}),
    )?;
    let input = json!({"x":"b"});
    let codes = |choices: &ValidationChoices| -> Result<Vec<String>, SchemaPointer> {
        Ok(parse(choices, &listed, &input)?
            .issues
            .iter()
            .filter_map(|issue| issue.get("code").and_then(Value::as_str).map(str::to_owned))
            .collect())
    };
    let declared = codes(&ValidationChoices::default()).map_err(|pointer| pointer.to_string())?;
    assert_eq!(declared, ["too_small", "invalid_format"]);
    let format_first = codes(&only(SchemaChoice::StringCheckOrder(vec![
        StringCheckKind::Format,
        StringCheckKind::Minimum,
        StringCheckKind::Maximum,
    ]))?)
    .map_err(|pointer| pointer.to_string())?;
    assert_eq!(format_first, ["invalid_format", "too_small"]);
    Ok(())
}

#[test]
fn refinements_and_transformations_run_at_the_node_with_zods_issue_shape() -> TestResult {
    let listed = schema(json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}))?;
    let callback = SchemaCallback::from(SchemaCallbackOptions {
        refine: Some(Arc::new(|value: &Value| {
            if value.as_str() == Some("bad") {
                Err(RefineFailure::invalid())
            } else {
                Ok(())
            }
        })),
        transform: Some(Arc::new(|value: Value| {
            Ok(Value::from(value.as_str().unwrap_or_default().to_uppercase()))
        })),
    });
    let choices = ValidationChoices::new(ValidationChoicesOptions {
        choices: Vec::new(),
        callbacks: vec![(pointer("/properties/x")?, callback)],
    })?;
    let bad = parse(&choices, &listed, &json!({"x":"bad"})).map_err(|pointer| pointer.to_string())?;
    assert_eq!(
        bad.issues,
        [schema(json!({"code":"custom","path":["x"],"message":"Invalid input"}))?]
    );
    let good = parse(&choices, &listed, &json!({"x":"ok"})).map_err(|pointer| pointer.to_string())?;
    assert!(good.issues.is_empty());
    assert_eq!(good.value, Some(json!({"x":"OK"})));
    Ok(())
}

#[test]
fn the_discriminated_union_choice_names_a_missing_discriminator() -> TestResult {
    let listed = schema(json!({"type":"object","properties":{"x":{"anyOf":[
        {"type":"object","properties":{"k":{"type":"string","const":"a"}},"required":["k"]},
        {"type":"object","properties":{"k":{"type":"string","const":"b"}},"required":["k"]}
    ]}},"required":["x"]}))?;
    let choices = only(SchemaChoice::Union(UnionChoice::Discriminated {
        discriminator: "k".into(),
    }))?;
    let parsed = parse(&choices, &listed, &json!({"x":{"k":"c"}})).map_err(|pointer| pointer.to_string())?;
    assert_eq!(
        parsed.issues,
        [schema(json!({
            "code":"invalid_union","errors":[],"note":"No matching discriminator","discriminator":"k",
            "path":["x","k"],"message":"Invalid input"
        }))?]
    );
    Ok(())
}
