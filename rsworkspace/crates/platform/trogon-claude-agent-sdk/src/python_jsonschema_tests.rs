#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests read the recorded jsonschema results from JSON and build schemas inline."
    )
)]

use super::ops::Index;
use super::*;
use crate::{
    python_json_load::python_json_loads,
    python_json_value::{PythonJsonObject, python_json_dumps},
    python_recursion_model::RecursionModelOptions,
};
use serde_json::{Value as Json, json};
use std::{borrow::Cow, collections::HashMap};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Every draft's errors, `best_match` outcome and exceptions, recorded from `jsonschema` 4.26 under several
/// `PYTHONHASHSEED` values.
const DRAFTS: &str = include_str!("../tests/fixtures/sdk_mcp/python_drafts.json");

/// Stack for the replay, so the deepest recorded schema reaches its result before the native stack runs out.
const REPLAY_STACK: usize = 256 << 20;

/// What the fixture records for an error field that is still unset; Python never yields one.
const UNSET: &str = "<unset>";

/// A value built inline with `json!`, as the engine reads it.
fn py(value: Json) -> Value {
    Value::from(value)
}

/// A Python `str` the engine raised, as a test error: quoted, with each surrogate escaped.
fn shown(error: PythonString) -> String {
    format!("{error:?}")
}

fn class_name(draft: Draft) -> &'static str {
    match draft {
        Draft::Draft3 => "Draft3Validator",
        Draft::Draft4 => "Draft4Validator",
        Draft::Draft6 => "Draft6Validator",
        Draft::Draft7 => "Draft7Validator",
        Draft::Draft201909 => "Draft201909Validator",
        Draft::Draft202012 => "Draft202012Validator",
    }
}

fn segment(segment: &PathSegment) -> Value {
    match segment {
        PathSegment::Key(key) => Value::String(key.clone()),
        PathSegment::Index(index) => Value::from(*index),
    }
}

/// The error as the generator encodes it: `message`, `list(path)`, `list(schema_path)`, `validator`,
/// `validator_value` and the `context`, recursively.
fn encode(error: &PyError) -> Value {
    let validator = match &error.validator {
        Some(Some(name)) => Value::from(name.as_str()),
        Some(None) => Value::Null,
        None => Value::from(UNSET),
    };
    Value::Object(PythonJsonObject::from_iter([
        (PythonString::from("message"), Value::from(error.message.as_str())),
        (
            PythonString::from("path"),
            Value::Array(error.path.iter().map(segment).collect()),
        ),
        (
            PythonString::from("schema_path"),
            Value::Array(error.schema_path.iter().map(segment).collect()),
        ),
        (PythonString::from("validator"), validator),
        (
            PythonString::from("validator_value"),
            error.validator_value.clone().unwrap_or_else(|| Value::from(UNSET)),
        ),
        (
            PythonString::from("context"),
            Value::Array(error.context.iter().map(encode).collect()),
        ),
    ]))
}

fn outcome_text(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Valid => "VALID".to_owned(),
        Outcome::Invalid(message) => format!("INVALID {message}"),
        Outcome::Raised(text) => format!("RAISED {}", text.to_str_lossy()),
        Outcome::Escaped(message) => format!("ESCAPED {message}"),
    }
}

/// The same errors, in the same order unless the fixture saw the order move with the hash seed.
fn same_errors(actual: &[Value], expected: &[Value], unordered: bool) -> bool {
    if !unordered {
        return actual == expected;
    }
    if actual.len() != expected.len() {
        return false;
    }
    let mut left: Vec<&Value> = expected.iter().collect();
    actual
        .iter()
        .all(|each| match left.iter().position(|other| *other == each) {
            Some(at) => {
                left.swap_remove(at);
                true
            }
            None => false,
        })
}

fn listed(record: &Json, field: &str, wanted: &Json) -> bool {
    record
        .get(field)
        .and_then(Json::as_array)
        .is_some_and(|all| all.contains(wanted))
}

/// Replays the fixture and returns how many instances it checked, or the first mismatches.
fn replay() -> Result<usize, String> {
    let fixture: Json = serde_json::from_str(DRAFTS).map_err(|error| error.to_string())?;
    let cases = fixture
        .get("cases")
        .and_then(Json::as_array)
        .ok_or("the fixture has no cases")?;
    let mut checked = 0;
    let mut failures = Vec::new();
    for case in cases {
        let schema = case.get("schema").ok_or("a case has no schema")?;
        let recorded = case.get("draft").and_then(Json::as_str).ok_or("a case has no draft")?;
        let instances = case
            .get("instances")
            .and_then(Json::as_array)
            .ok_or("a case has no instances")?;
        let engine_schema = Value::from(schema);
        let chosen = drafts::validator_for(&engine_schema, Draft::Draft202012);
        let draft = match (chosen, recorded.strip_prefix("RAISE ")) {
            (Ok(draft), None) if class_name(draft) == recorded => Some(draft),
            (Err(text), Some(expected)) if text.as_str() == Some(expected) => None,
            (other, _) => {
                failures.push(format!("{schema}: validator_for gave {other:?}, Python {recorded}"));
                continue;
            }
        };
        for record in instances {
            let instance = record.get("instance").ok_or("a record has no instance")?;
            let engine_instance = Value::from(instance);
            checked += 1;
            // The fixture records `best_match` without `check_schema`, so only `validator_for` raising goes through
            // `validate`.
            let outcome = outcome_text(&match draft {
                Some(draft) => best_match(draft, &engine_schema, &engine_instance),
                None => validate(&engine_schema, &engine_instance),
            });
            let Some(draft) = draft else {
                let expected = format!("RAISED {}", recorded.trim_start_matches("RAISE "));
                if outcome != expected {
                    failures.push(format!("{schema} with {instance}: {outcome}, Python {expected}"));
                }
                continue;
            };
            let (errors, raised) = iter_errors(draft, &engine_schema, &engine_instance);
            let errors: Vec<Value> = errors.iter().map(encode).collect();
            let unordered = record.get("unordered").and_then(Json::as_bool).unwrap_or(false);
            let variants = record
                .get("errors")
                .and_then(Json::as_array)
                .ok_or("a record has no errors")?;
            let errors_match = variants.iter().any(|variant| {
                variant.as_array().is_some_and(|expected| {
                    let expected: Vec<Value> = expected.iter().map(Value::from).collect();
                    same_errors(&errors, &expected, unordered)
                })
            });
            let raised = raised.map_or(Json::Null, |text| Json::String(text.to_str_lossy().into_owned()));
            let raised_match = listed(record, "raised", &raised);
            let outcome_match = listed(record, "outcome", &Json::String(outcome.clone()));
            if !(errors_match && raised_match && outcome_match) {
                failures.push(format!(
                    "{schema} with {instance}: errors {} raised {raised} outcome {outcome}; Python {record}",
                    python_json_dumps(&Value::Array(errors))
                ));
            }
        }
    }
    if failures.is_empty() {
        return Ok(checked);
    }
    let shown: Vec<String> = failures.iter().take(20).cloned().collect();
    Err(format!(
        "{} mismatches, the first ones:\n{}",
        failures.len(),
        shown.join("\n")
    ))
}

#[test]
fn every_draft_yields_the_errors_and_outcome_jsonschema_does() -> TestResult {
    let checked = std::thread::Builder::new()
        .stack_size(REPLAY_STACK)
        .spawn(replay)?
        .join()
        .map_err(|_| "the replay thread panicked")??;
    assert!(checked > 14_000, "only {checked} instances were checked");
    Ok(())
}

#[test]
fn the_fixture_names_every_case_it_could_not_record() -> TestResult {
    let fixture: Json = serde_json::from_str(DRAFTS)?;
    let skipped = fixture
        .get("skipped")
        .and_then(Json::as_array)
        .ok_or("no skipped list")?;
    assert!(!skipped.is_empty());
    for each in skipped {
        let reason = each.get("reason").and_then(Json::as_str).unwrap_or_default();
        assert!(
            reason.starts_with("int out of serde_json range")
                || reason.starts_with("float serde_json reads differently"),
            "{each}"
        );
    }
    Ok(())
}

#[test]
fn validate_reports_the_best_match_or_the_exception() {
    assert_eq!(
        validate(&py(json!({"type": "string"})), &py(json!("a"))),
        Outcome::Valid
    );
    assert_eq!(
        validate(&py(json!({"type": "string"})), &py(json!(5))),
        Outcome::Invalid("5 is not of type 'string'".to_owned())
    );
    assert_eq!(
        validate(&py(json!({"pattern": "["})), &py(json!("a"))),
        Outcome::Raised(
            "'[' is not a 'regex'\n\nFailed validating 'format' in metaschema['allOf'][3]['properties']['pattern']:\n    \
             {'type': 'string', 'format': 'regex'}\n\nOn schema['pattern']:\n    '['"
                .into()
        )
    );
    assert_eq!(
        best_match(Draft::Draft202012, &py(json!({"pattern": "["})), &py(json!("a"))),
        Outcome::Raised("unterminated character set at position 0".into())
    );
}

#[test]
fn where_no_validation_thread_runs_the_caller_validates_inline() {
    let model = RecursionModel::default();
    for (schema, instance) in [
        (py(json!({"type": "string"})), py(json!("a"))),
        (py(json!({"type": "string"})), py(json!(5))),
        (py(json!({"pattern": "["})), py(json!("a"))),
    ] {
        assert_eq!(
            validate_on(
                &[],
                &schema,
                &instance,
                &model,
                &NoRetrieval,
                &Mutex::default(),
                HashSecret::default()
            ),
            validate(&schema, &instance)
        );
    }
}

const DRAFTS_BY_URI: [(Draft, &str); 6] = [
    (Draft::Draft3, "http://json-schema.org/draft-03/schema#"),
    (Draft::Draft4, "http://json-schema.org/draft-04/schema#"),
    (Draft::Draft6, "http://json-schema.org/draft-06/schema#"),
    (Draft::Draft7, "http://json-schema.org/draft-07/schema#"),
    (Draft::Draft201909, "https://json-schema.org/draft/2019-09/schema"),
    (Draft::Draft202012, "https://json-schema.org/draft/2020-12/schema"),
];

#[test]
fn check_schema_accepts_a_valid_schema_and_raises_what_re_compile_raises_for_every_draft() {
    for (draft, uri) in DRAFTS_BY_URI {
        assert_eq!(
            check_schema(draft, &py(json!({"$schema": uri, "type": "object"}))),
            Ok(()),
            "{uri}"
        );
        assert_eq!(
            check_schema(draft, &py(json!({"$schema": uri, "pattern": "a{99999999999}"}))),
            Err(Outcome::Raised("the repetition number is too large".into())),
            "{uri}"
        );
    }
}

#[test]
fn check_schema_raises_the_first_metaschema_error_not_the_best_match() -> TestResult {
    let schema = |uri: &str| py(json!({"properties": {"a": {"type": "nope"}}, "required": "a", "$schema": uri}));
    let expected = [
        (
            Draft::Draft3,
            "'a' is not of type 'boolean'\n\nFailed validating 'type' in metaschema['properties']['required']:\n    \
             {'type': 'boolean', 'default': False}\n\nOn schema['required']:\n    'a'",
        ),
        (
            Draft::Draft4,
            "'a' is not of type 'array'\n\nFailed validating 'type' in metaschema['properties']['required']:\n    \
             {'type': 'array',\n     'items': {'type': 'string'},\n     'minItems': 1,\n     'uniqueItems': True}\n\n\
             On schema['required']:\n    'a'",
        ),
        (
            Draft::Draft201909,
            "'nope' is not valid under any of the given schemas\n\nFailed validating 'anyOf' in \
             metaschema['allOf'][1]['properties']['properties']['additionalProperties']['$recursiveRef']['allOf'][2]\
             ['properties']['type']:\n    {'anyOf': [{'$ref': '#/$defs/simpleTypes'},\n               \
             {'type': 'array',\n                'items': {'$ref': '#/$defs/simpleTypes'},\n                \
             'minItems': 1,\n                'uniqueItems': True}]}\n\nOn schema['properties']['a']['type']:\n    'nope'",
        ),
        (
            Draft::Draft202012,
            "'nope' is not valid under any of the given schemas\n\nFailed validating 'anyOf' in \
             metaschema['allOf'][1]['properties']['properties']['additionalProperties']['$dynamicRef']['allOf'][3]\
             ['properties']['type']:\n    {'anyOf': [{'$ref': '#/$defs/simpleTypes'},\n               \
             {'type': 'array',\n                'items': {'$ref': '#/$defs/simpleTypes'},\n                \
             'minItems': 1,\n                'uniqueItems': True}]}\n\nOn schema['properties']['a']['type']:\n    'nope'",
        ),
    ];
    for (draft, text) in expected {
        let schema = schema(draft.dialect());
        assert_eq!(
            check_schema(draft, &schema),
            Err(Outcome::Raised(text.into())),
            "{draft:?}"
        );
    }
    // The best match of the same errors is another one.
    let metaschema = referencing::metaschema(Draft::Draft202012).map_err(shown)?;
    assert_eq!(
        best_match(Draft::Draft202012, metaschema, &schema(Draft::Draft202012.dialect())),
        Outcome::Invalid("'a' is not of type 'array'".to_owned())
    );
    Ok(())
}

#[test]
fn a_schema_error_prints_like_python_with_fields_unset_or_a_false_schema() {
    assert_eq!(
        schema_error(&PyError::new("boom".to_owned()), &Budget::default()),
        Outcome::Raised("boom".into())
    );
    let mut error = PyError::new("False schema does not allow 1".to_owned());
    error.validator = Some(None);
    error.validator_value = Some(Value::Null);
    error.instance = Some(py(json!(1)));
    error.schema = Some(py(json!(false)));
    assert_eq!(
        schema_error(&error, &Budget::default()),
        Outcome::Raised(
            "False schema does not allow 1\n\nFailed validating None in metaschema:\n    False\n\nOn schema:\n    1"
                .into()
        )
    );
}

/// The errors `cls(schema, format_checker=...)` yields for `instance`, or the exception that stopped them.
fn format_errors(schema: &Value, instance: &Value, format_checker: bool) -> Result<Vec<String>, String> {
    let mut sink = Sink::new(false);
    let mut budget = Budget::default();
    match run(
        Draft::Draft202012,
        schema,
        instance,
        format_checker,
        &mut sink,
        &NoRetrieval,
        &mut budget,
    ) {
        Ok(()) | Err(Stop::Done) => Ok(sink.errors.iter().map(|error| error.message.clone()).collect()),
        Err(Stop::Raise(text)) => Err(text.to_str_lossy().into_owned()),
    }
}

#[test]
fn format_checks_only_regex_and_only_with_a_format_checker() {
    let regex = py(json!({"format": "regex"}));
    assert_eq!(
        format_errors(&regex, &py(json!("(")), true),
        Ok(vec!["'(' is not a 'regex'".to_owned()])
    );
    assert_eq!(format_errors(&regex, &py(json!("a")), true), Ok(Vec::new()));
    assert_eq!(format_errors(&regex, &py(json!(5)), true), Ok(Vec::new()));
    assert_eq!(format_errors(&regex, &py(json!("(")), false), Ok(Vec::new()));
    assert_eq!(
        format_errors(&py(json!({"format": "uri"})), &py(json!("::")), true),
        Ok(Vec::new())
    );
    assert_eq!(
        format_errors(&py(json!({"format": []})), &py(json!("x")), true),
        Err("unhashable type: 'list'".to_owned())
    );
    assert_eq!(
        format_errors(&py(json!({"format": []})), &py(json!("x")), false),
        Ok(Vec::new())
    );
}

/// Every `format` value under a `format` key and every `false` with the key that holds it.
fn walk<'v>(value: &'v Json, key: &str, formats: &mut Vec<&'v str>, falses: &mut Vec<String>) {
    match value {
        Json::Object(map) => {
            if let Some(Json::String(format)) = map.get("format") {
                formats.push(format);
            }
            for (name, each) in map {
                walk(each, name, formats, falses);
            }
        }
        Json::Array(items) => items.iter().for_each(|each| walk(each, key, formats, falses)),
        Json::Bool(false) => falses.push(key.to_owned()),
        _ => {}
    }
}

/// `check_schema` checks formats only on metaschema subschemas, so the formats it can meet are the ones the
/// metaschemas name, and no metaschema error comes from a false schema.
#[test]
fn the_metaschemas_use_only_regex_and_uri_formats_and_no_false_schema() -> TestResult {
    let mut formats = Vec::new();
    let mut falses = Vec::new();
    let documents = crate::constants::METASCHEMAS
        .iter()
        .map(|source| serde_json::from_str::<Json>(source))
        .collect::<Result<Vec<_>, _>>()?;
    for document in &documents {
        walk(document, "", &mut formats, &mut falses);
    }
    formats.sort_unstable();
    formats.dedup();
    assert_eq!(formats, ["regex", "uri", "uri-reference"]);
    assert!(!falses.is_empty());
    for key in &falses {
        assert!(
            key == "default" || key.starts_with("https://json-schema.org/draft/2019-09/vocab/"),
            "{key}"
        );
    }
    Ok(())
}

#[test]
fn every_draft_has_its_metaschema_and_a_registry_without_it_raises() -> TestResult {
    for (draft, _) in DRAFTS_BY_URI {
        let metaschema = referencing::metaschema(draft).map_err(shown)?;
        assert_eq!(drafts::validator_for(metaschema, Draft::Draft202012), Ok(draft));
    }
    assert_eq!(
        referencing::Crawled::default().metaschema(Draft::Draft7),
        Err(PythonString::from("KeyError: 'http://json-schema.org/draft-07/schema'"))
    );
    Ok(())
}

#[test]
fn a_reference_cycle_raises_once_the_recursion_limit_is_passed() {
    for schema in [
        py(json!({"$ref": "#"})),
        py(json!({"$schema": "http://json-schema.org/draft-07/schema#", "$ref": "#"})),
        py(json!({"$schema": "https://json-schema.org/draft/2019-09/schema#", "$recursiveRef": "#"})),
    ] {
        assert_eq!(
            validate(&schema, &py(json!(1))),
            Outcome::Raised("maximum recursion depth exceeded".into()),
            "{schema:?}"
        );
    }
}

/// Recursion boundaries of every keyword shape and raise site, recorded from the pinned Python package's tool call
/// at the nesting just inside and just past the limit, warm and in a fresh process.
const RECURSION: &str = include_str!("../tests/fixtures/sdk_mcp/python_recursion.json");

/// The same for patterns whose groups nest until `re` passes the limit.
const RECURSION_REGEX: &str = include_str!("../tests/fixtures/sdk_mcp/python_recursion_regex.json");

/// The same for a type check at the end of a recursive draft 3 schema under the recursion limits 1000 to 1009, each
/// case with the `limit` it ran under.
const RECURSION_LIMITS: &str = include_str!("../tests/fixtures/sdk_mcp/python_recursion_limits.json");

/// Stack for the recursion replays, whose fixtures nest about ten thousand levels deep.
const DEEP_REPLAY_STACK: usize = 512 << 20;

/// A fixture read past the recursion limit of `serde_json`.
fn deep_fixture(text: &str) -> Result<Json, String> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    let fixture =
        serde::Deserialize::deserialize(&mut deserializer).map_err(|error: serde_json::Error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
    Ok(fixture)
}

/// The answer of the bridge's tool call for `outcome`, in the fixture's `OK`, `ERR` and `RPCERR` text.
fn bridge_answer(outcome: Outcome) -> String {
    match outcome {
        Outcome::Valid => "OK".to_owned(),
        Outcome::Invalid(message) => format!("ERR Input validation error: {message}"),
        Outcome::Raised(text) => format!("ERR {}", text.to_str_lossy()),
        Outcome::Escaped(message) => format!(r#"RPCERR {{"code": 0, "message": "{message}"}}"#),
    }
}

/// The start of `text`, so a mismatch on a value nested thousands of levels deep stays readable.
fn clipped(text: &str) -> String {
    text.chars().take(160).collect()
}

/// The model of a call at `jsonschema.validate`'s depth on the bridge path under `limit`.
fn bridge_model(cache: CacheState, limit: usize) -> Result<RecursionModel, String> {
    RecursionModel::new(RecursionModelOptions {
        caller_depth: 17,
        cache,
        limit,
    })
    .map_err(|error| error.to_string())
}

/// Replays a recursion fixture with a warm model and with a cold one, the `cold_answers` of a case standing for its
/// `answers` where a fresh process answers differently, under the case's `limit` or the default one. Returns how many
/// calls it checked, or the mismatches.
fn replay_recursion(text: &str) -> Result<usize, String> {
    let fixture = deep_fixture(text)?;
    let cases = fixture
        .get("cases")
        .and_then(Json::as_array)
        .ok_or("the fixture has no cases")?;
    let mut checked = 0;
    let mut failures = Vec::new();
    for case in cases {
        let name = case.get("name").and_then(Json::as_str).ok_or("a case has no name")?;
        let limit = match case.get("limit") {
            Some(limit) => limit
                .as_u64()
                .and_then(|limit| usize::try_from(limit).ok())
                .ok_or("a limit is not a count")?,
            None => 1000,
        };
        let warm = bridge_model(CacheState::Warm, limit)?;
        let cold = bridge_model(CacheState::Cold, limit)?;
        let schema = Value::from(case.get("schema").ok_or("a case has no schema")?);
        let arguments = case
            .get("arguments")
            .and_then(Json::as_array)
            .ok_or("a case has no arguments")?;
        let answers = case
            .get("answers")
            .and_then(Json::as_array)
            .ok_or("a case has no answers")?;
        let cold_answers = case.get("cold_answers").and_then(Json::as_array).unwrap_or(answers);
        if arguments.len() != answers.len() || answers.len() != cold_answers.len() {
            return Err(format!("{name}: the arguments and answers differ in length"));
        }
        for ((instance, warm_answer), cold_answer) in arguments.iter().zip(answers).zip(cold_answers) {
            let instance = Value::from(instance);
            for (model, expected) in [(&warm, warm_answer), (&cold, cold_answer)] {
                checked += 1;
                let actual = bridge_answer(validate_with(
                    &schema,
                    &instance,
                    model,
                    &NoRetrieval,
                    &Mutex::default(),
                    HashSecret::default(),
                ));
                let expected = expected.as_str().ok_or("an answer is not text")?;
                if actual != expected {
                    failures.push(format!(
                        "{name} ({:?}): {}, Python {}",
                        model.cache(),
                        clipped(&actual),
                        clipped(expected)
                    ));
                }
            }
        }
    }
    if failures.is_empty() {
        return Ok(checked);
    }
    Err(format!("{} mismatches:\n{}", failures.len(), failures.join("\n")))
}

/// [`replay_recursion`] on a thread whose stack holds the fixture; returns how many calls it checked.
fn replay_recursion_deep(text: &'static str) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(std::thread::Builder::new()
        .stack_size(DEEP_REPLAY_STACK)
        .spawn(move || replay_recursion(text))?
        .join()
        .map_err(|_| "the replay thread panicked")??)
}

#[test]
fn every_recursion_boundary_falls_where_python_raises_warm_and_cold() -> TestResult {
    assert_eq!(replay_recursion_deep(RECURSION)?, 2 * 444);
    Ok(())
}

#[test]
fn every_regex_recursion_boundary_falls_where_python_raises_warm_and_cold() -> TestResult {
    assert_eq!(replay_recursion_deep(RECURSION_REGEX)?, 2 * 76);
    Ok(())
}

#[test]
fn every_recursion_boundary_under_a_raised_limit_falls_where_python_raises_warm_and_cold() -> TestResult {
    assert_eq!(replay_recursion_deep(RECURSION_LIMITS)?, 2 * 40);
    Ok(())
}

#[test]
fn a_schema_error_too_deep_to_print_escapes_the_handler() {
    let schema_with = |nesting: usize| {
        let mut minimum = py(json!(1));
        for _ in 0..nesting {
            minimum = Value::Array(vec![minimum]);
        }
        let property = Value::Object(PythonJsonObject::from_iter([(PythonString::from("minimum"), minimum)]));
        let properties = Value::Object(PythonJsonObject::from_iter([(PythonString::from("v"), property)]));
        Value::Object(PythonJsonObject::from_iter([
            (PythonString::from("type"), py(json!("object"))),
            (PythonString::from("properties"), properties),
        ]))
    };
    let printed = outcome_text(&validate(&schema_with(325), &py(json!({"v": 1}))));
    assert!(printed.starts_with("RAISED [[[["), "{}", clipped(&printed));
    assert_eq!(
        outcome_text(&validate(&schema_with(326), &py(json!({"v": 1})))),
        "ESCAPED maximum recursion depth exceeded"
    );
}

#[test]
fn a_value_nested_past_the_c_repr_limit_is_not_shown() {
    let mut value = py(json!(1));
    for _ in 0..9991 {
        value = Value::Array(vec![value]);
    }
    assert_eq!(ops::repr_depth(&value), 9992);
    assert_eq!(
        ops::shown(&value, VALIDATE_HELD),
        Err("maximum recursion depth exceeded while getting the repr of an object".to_owned())
    );
    assert_eq!(
        ops::shown(&value, VALIDATE_HELD - 1).map(|text| text.len()),
        Ok(2 * 9991 + 1)
    );
}

#[test]
fn a_repr_call_and_a_joined_repr_meet_the_c_repr_limit_sooner() {
    let overflow = Err("maximum recursion depth exceeded while getting the repr of an object".to_owned());
    for (depth, shown, called, joined) in [
        (9987, true, true, true),
        (9988, true, true, false),
        (9990, true, true, false),
        (9991, true, false, false),
    ] {
        let mut value = py(json!(1));
        for _ in 1..depth {
            value = Value::Array(vec![value]);
        }
        assert_eq!(ops::repr_depth(&value), depth);
        assert_eq!(ops::shown(&value, VALIDATE_HELD).is_ok(), shown, "{depth}");
        assert_eq!(ops::repr_called(&value, VALIDATE_HELD).is_ok(), called, "{depth}");
        assert_eq!(ops::join_reprs([&value], VALIDATE_HELD).is_ok(), joined, "{depth}");
        assert_eq!(ops::extras_msg([&value], VALIDATE_HELD).is_ok(), joined, "{depth}");
        if !called {
            assert_eq!(ops::repr_called(&value, VALIDATE_HELD), overflow);
        }
    }
}

#[test]
fn a_pattern_that_is_not_a_string_raises_what_re_search_raises() {
    for (pattern, raised) in [
        (json!(5), "first argument must be string or compiled pattern"),
        (json!(null), "first argument must be string or compiled pattern"),
        (json!([1]), "unhashable type: 'list'"),
        (json!({"a": 1}), "unhashable type: 'dict'"),
    ] {
        assert_eq!(
            best_match(Draft::Draft202012, &py(json!({"pattern": pattern})), &py(json!("a"))),
            Outcome::Raised(raised.into()),
            "{pattern}"
        );
    }
}

fn key(path: &VecDeque<PathSegment>) -> Key<'_> {
    Key {
        path,
        strong: true,
        mismatch: true,
    }
}

#[test]
fn relevance_keys_raise_where_python_cannot_order_a_str_and_an_int() {
    let text = VecDeque::from([PathSegment::Key("a".into())]);
    let index = VecDeque::from([PathSegment::Index(0)]);
    assert_eq!(
        exceeds(&key(&text), Op::Lt, &key(&index)),
        Err("'<' not supported between instances of 'str' and 'int'".to_owned())
    );
    assert_eq!(
        exceeds(&key(&index), Op::Gt, &key(&text)),
        Err("'>' not supported between instances of 'int' and 'str'".to_owned())
    );
    assert_eq!(exceeds(&key(&text), Op::Lt, &key(&text)), Ok(false));
}

#[test]
fn the_metaschema_registry_holds_what_jsonschema_specifications_crawls() -> TestResult {
    let crawled = referencing::SPECIFICATIONS
        .as_ref()
        .map_err(|error| shown(error.clone()))?;
    assert_eq!(crawled.resource_count(), 20);
    assert_eq!(crawled.dynamic_anchor_count(), 9);
    Ok(())
}

#[test]
fn a_metaschema_source_without_a_known_dialect_or_an_id_is_rejected() {
    assert!(referencing::specifications(&["{"], HashSecret::default()).is_err());
    assert_eq!(
        referencing::specifications(&["{}"], HashSecret::default()).err(),
        Some(PythonString::from("Could not determine the specification of {}"))
    );
    assert_eq!(
        referencing::specifications(
            &[r#"{"$schema": "http://json-schema.org/draft-07/schema#"}"#],
            HashSecret::default()
        )
        .err(),
        Some(PythonString::from(
            "{'$schema': 'http://json-schema.org/draft-07/schema#'} has no internal id"
        ))
    );
}

#[test]
fn an_anchor_is_found_under_the_canonical_id_of_the_resource_a_uri_names() -> TestResult {
    let contents = py(json!({"$id": "urn:real", "$anchor": "a", "type": "string"}));
    let root = Resource {
        contents: &contents,
        draft: Draft::Draft202012,
    };
    let documents = referencing::Documents::default();
    let specifications = referencing::SPECIFICATIONS
        .as_ref()
        .map_err(|error| shown(error.clone()))?;
    let registry = Registry::new(
        specifications,
        "urn:alias".into(),
        root,
        &NoRetrieval,
        &documents,
        HashSecret::default(),
    );
    let resolved = Resolver::new("urn:alias".into())
        .lookup(&registry, &py(json!("#a")))
        .map_err(|error| shown(error.raw()))?;
    assert_eq!(*resolved.contents, contents);
    Ok(())
}

#[test]
fn sorting_mixed_values_raises_the_type_error_python_names() {
    let mixed = vec![Cow::Owned(py(json!(1))), Cow::Owned(py(json!("a")))];
    assert_eq!(
        ops::sorted_values(mixed).err(),
        Some("'<' not supported between instances of 'str' and 'int'".to_owned())
    );
    let names = vec![Cow::Owned(py(json!("b"))), Cow::Owned(py(json!("a")))];
    let sorted = ops::sorted_values(names).map(|values| values.into_iter().map(Cow::into_owned).collect::<Vec<_>>());
    assert_eq!(sorted, Ok(vec![py(json!("a")), py(json!("b"))]));
}

#[test]
fn value_operations_raise_what_cpython_raises() {
    assert_eq!(
        ops::len(&py(json!(5))),
        Err("object of type 'int' has no len()".to_owned())
    );
    assert_eq!(ops::len(&py(json!("日本"))), Ok(2));
    assert_eq!(ops::len(&py(json!({"a": 1, "b": 2}))), Ok(2));
    assert_eq!(
        ops::compare(&py(json!(1)), ops::Op::Le, &py(json!("a"))),
        Err("'<=' not supported between instances of 'int' and 'str'".to_owned())
    );
    for (value, truthy) in [
        (json!(""), false),
        (json!("a"), true),
        (json!([]), false),
        (json!([1]), true),
        (json!({}), false),
        (json!({"a": 1}), true),
    ] {
        assert_eq!(ops::truthy(&py(value)), truthy);
    }
    assert_eq!(
        ops::join_patterns(&py(json!(["a", 5]))),
        Err("sequence item 1: expected str instance, int found".to_owned())
    );
    assert_eq!(
        ops::join_patterns(&py(json!(5))),
        Err("can only join an iterable".to_owned())
    );
    assert_eq!(
        ops::join_patterns(&py(json!({"a": 1, "b": 2}))),
        Ok(PythonString::from("a|b"))
    );
    assert_eq!(ops::join_patterns(&py(json!("ab"))), Ok(PythonString::from("a|b")));
    assert_eq!(ops::repr_depth(&py(json!(5))), 1);
    assert_eq!(ops::repr_depth(&py(json!([]))), 1);
    assert_eq!(ops::repr_depth(&py(json!({"a": [[]], "b": 1}))), 3);
    assert_eq!(
        ops::shown(&py(json!({"a": [1]})), VALIDATE_HELD).as_deref(),
        Ok("{'a': [1]}")
    );
    assert_eq!(
        ops::join_reprs(&[py(json!("a")), py(json!(1))], VALIDATE_HELD).as_deref(),
        Ok("'a', 1")
    );
    assert_eq!(
        ops::extras_msg(&[py(json!("a"))], VALIDATE_HELD),
        Ok(("'a'".to_owned(), "was"))
    );
    assert_eq!(ops::contains(&py(json!("abc")), &py(json!("b"))), Ok(true));
    assert_eq!(
        ops::contains(&py(json!("abc")), &py(json!(5))),
        Err("'in <string>' requires string as left operand, not int".to_owned())
    );
    assert_eq!(
        ops::contains(&py(json!(5)), &py(json!("a"))),
        Err("argument of type 'int' is not iterable".to_owned())
    );
    assert_eq!(
        ops::contains(&py(json!({"a": 1})), &py(json!([1]))),
        Err("unhashable type: 'list'".to_owned())
    );
    assert_eq!(
        ops::compare(&py(json!(1)), Op::Ge, &py(json!("a"))),
        Err("'>=' not supported between instances of 'int' and 'str'".to_owned())
    );
}

#[test]
fn pointer_indexes_parse_like_python_int() {
    assert_eq!(ops::parse_int(" \u{2003}\u{0661}\u{0662} "), Ok(Index::Small(12)));
    assert_eq!(ops::parse_int("-1_0"), Ok(Index::Small(-10)));
    assert_eq!(ops::parse_int("+7"), Ok(Index::Small(7)));
    assert_eq!(
        ops::parse_int("99999999999999999999999999999999999999999"),
        Ok(Index::Huge)
    );
    assert_eq!(
        ops::parse_int("1_"),
        Err("invalid literal for int() with base 10: '1_'".to_owned())
    );
    assert_eq!(
        ops::parse_int("\u{e9}"),
        Err("invalid literal for int() with base 10: '\u{e9}'".to_owned())
    );
    assert_eq!(
        ops::parse_int(&"1".repeat(4301)),
        Err("Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit".to_owned())
    );
    assert_eq!(ops::position(Index::Small(-1), 3), Some(2));
    assert_eq!(ops::position(Index::Small(-4), 3), None);
    assert_eq!(ops::position(Index::Small(3), 3), None);
    assert_eq!(ops::position(Index::Huge, 3), None);
}

/// The keyword sets, the `additionalProperties` extras and the crawl of duplicate ids and anchors, recorded from
/// `jsonschema` 4.26 and `referencing` 0.37 in Python processes started with several `PYTHONHASHSEED` values.
const SET_ORDER: &str = include_str!("../tests/fixtures/sdk_mcp/python_set_order.json");

fn seeded(record: &Json) -> Result<HashSecret, Box<dyn std::error::Error>> {
    let seed = record
        .get("seed")
        .and_then(Json::as_u64)
        .ok_or("a seed is not a number")?;
    Ok(HashSecret::from_seed(u32::try_from(seed)?))
}

fn named(fixture: &Json, name: &str) -> Result<Vec<Json>, String> {
    fixture
        .get(name)
        .and_then(Json::as_array)
        .cloned()
        .ok_or_else(|| format!("{name} is not a list"))
}

#[test]
fn the_keyword_sets_of_every_specification_iterate_like_referencing_under_every_recorded_seed() -> TestResult {
    let fixture: Json = serde_json::from_str(SET_ORDER)?;
    let drafts = [
        ("DRAFT3", Draft::Draft3),
        ("DRAFT4", Draft::Draft4),
        ("DRAFT6", Draft::Draft6),
        ("DRAFT7", Draft::Draft7),
        ("DRAFT201909", Draft::Draft201909),
        ("DRAFT202012", Draft::Draft202012),
    ];
    for record in named(&fixture, "seeds")? {
        let secret = seeded(&record)?;
        for (name, draft) in drafts {
            let specification = draft.specification();
            let recorded = record
                .pointer(&format!("/specifications/{name}"))
                .ok_or("a draft is missing")?;
            for (field, keys) in [
                ("in_value", specification.in_value),
                ("in_subarray", specification.in_subarray),
                ("in_subvalues", specification.in_subvalues),
            ] {
                let actual: Vec<Json> = crate::python_set::display_order(keys, secret)
                    .into_iter()
                    .map(|at| Json::from(keys[at]))
                    .collect();
                assert_eq!(
                    Some(&Json::Array(actual)),
                    recorded.get(field),
                    "{name} {field} seed {:?}",
                    record.get("seed")
                );
            }
        }
    }
    Ok(())
}

#[test]
fn iter_errors_follows_the_set_orders_of_the_python_process_under_every_recorded_seed() -> TestResult {
    let fixture: Json = serde_json::from_str(SET_ORDER)?;
    let cases = named(&fixture, "engine")?;
    let seeds = named(&fixture, "seeds")?;
    assert_eq!((cases.len(), seeds.len()), (10, 11));
    for record in seeds {
        let secret = seeded(&record)?;
        let recorded = named(&record, "engine")?;
        assert_eq!(recorded.len(), cases.len());
        for (case, expected) in cases.iter().zip(&recorded) {
            let schema = case.get("schema").ok_or("a case has no schema")?;
            let engine_schema = Value::from(schema);
            let instance = Value::from(case.get("instance").ok_or("a case has no instance")?);
            let draft = drafts::validator_for(&engine_schema, Draft::Draft202012).map_err(shown)?;
            let mut budget = Budget {
                secret,
                ..Budget::default()
            };
            let (errors, raised) = iter_errors_spending(draft, &engine_schema, &instance, &mut budget, &NoRetrieval);
            let errors = Value::Array(errors.iter().map(encode).collect());
            let raised = raised.map_or(Json::Null, |text| Json::String(text.to_str_lossy().into_owned()));
            assert_eq!(
                Some(errors),
                expected.get("errors").map(Value::from),
                "{schema} seed {:?}",
                record.get("seed")
            );
            assert_eq!(
                Some(&raised),
                expected.get("raised"),
                "{schema} seed {:?}",
                record.get("seed")
            );
        }
    }
    Ok(())
}

/// The pattern `textwrap.dedent` substitutes for the twelve-space margin of an error's text.
const MARGIN: &str = "(?m)^            ";

/// A 100-character string nested `depth` lists deep.
fn long_nest(depth: usize) -> Value {
    (0..depth).fold(py(json!("x".repeat(100))), |inner, _| Value::Array(vec![inner]))
}

/// [`printed`] of `UnknownType('nope', instance, schema)`.
fn print_unknown(
    model: &RecursionModel,
    patterns: &RefCell<ReCache>,
    schema: &Value,
    instance: &Value,
) -> Option<String> {
    printed(model, patterns, schema, instance, || {
        unknown_type_text("'nope'", schema, instance)
    })
}

/// Which of the word and margin patterns `patterns` holds.
fn holds(patterns: &RefCell<ReCache>) -> (bool, bool) {
    let patterns = patterns.borrow();
    (patterns.contains(WORDS), patterns.contains(MARGIN))
}

#[test]
fn printing_an_error_compiles_the_word_and_margin_patterns_as_python_does() -> TestResult {
    let run = || -> Result<(), String> {
        let model = RecursionModel::default();
        let instance = py(json!(1));
        let long = py(json!("x".repeat(100)));
        let patterns = RefCell::new(ReCache::default());
        assert_eq!(
            print_unknown(&model, &patterns, &long, &instance),
            Some(unknown_type_text("'nope'", &long, &instance).text)
        );
        assert_eq!(holds(&patterns), (true, true));

        // Compiling the word pattern at nesting 324 passes the limit before either pattern is stored.
        let deep = long_nest(324);
        let patterns = RefCell::new(ReCache::default());
        assert_eq!(print_unknown(&model, &patterns, &deep, &instance), None);
        assert_eq!(holds(&patterns), (false, false));
        // With the word pattern cached the lookup stays shallow enough, and dedent compiles the margin.
        patterns.borrow_mut().compiled(WORDS);
        assert_eq!(
            print_unknown(&model, &patterns, &deep, &instance),
            Some(unknown_type_text("'nope'", &deep, &instance).text)
        );
        assert_eq!(holds(&patterns), (true, true));
        // Both cached, the next print only looks them up.
        assert!(print_unknown(&model, &patterns, &deep, &instance).is_some());
        assert_eq!(holds(&patterns), (true, true));

        // A split before the overflow stores the word pattern although the print fails.
        let split_first = py(json!([
            "x".repeat(100),
            (0..340).fold(json!(1), |inner, _| Json::Array(vec![inner]))
        ]));
        let patterns = RefCell::new(ReCache::default());
        assert_eq!(print_unknown(&model, &patterns, &split_first, &instance), None);
        assert_eq!(holds(&patterns), (true, false));

        // A value whose plain repr passes the limit fails before it is printed.
        let too_deep = (0..500).fold(py(json!("x".repeat(100))), |inner, _| Value::Array(vec![inner]));
        let patterns = RefCell::new(ReCache::default());
        assert_eq!(print_unknown(&model, &patterns, &instance, &too_deep), None);
        assert_eq!(holds(&patterns), (false, false));
        Ok(())
    };
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(run)?
        .join()
        .map_err(|_| "the print thread panicked")??;
    Ok(())
}

#[test]
fn compiling_the_margin_pattern_needs_eight_frames_below_the_substitution() -> TestResult {
    let instance = py(json!(1));
    // `re.sub` runs two frames below `__str__`, which runs at 17; printing a scalar reaches 24.
    for (limit, prints) in [(26, false), (27, true)] {
        let model = bridge_model(CacheState::Warm, limit)?;
        let patterns = RefCell::new(ReCache::default());
        assert_eq!(
            print_unknown(&model, &patterns, &instance, &instance).is_some(),
            prints,
            "{limit}"
        );
        assert_eq!(holds(&patterns), (false, prints), "{limit}");
    }
    Ok(())
}

#[test]
fn a_text_without_a_margin_substitutes_nothing() {
    let model = RecursionModel::default();
    let patterns = RefCell::new(ReCache::default());
    let value = py(json!(1));
    let text = || {
        schema_error_text(
            "a\nb",
            Some("minimum"),
            &[],
            "metaschema",
            &value,
            &[],
            "schema",
            &value,
        )
    };
    let expected = text().text;
    assert_eq!(printed(&model, &patterns, &value, &value, text), Some(expected));
    assert_eq!(holds(&patterns), (false, false));
}

#[test]
fn an_unknown_type_printing_a_long_string_escapes_until_the_word_pattern_is_cached() -> TestResult {
    let schema = py(json!({
        "$schema": "http://json-schema.org/draft-03/schema#",
        "type": "object",
        "properties": {"v": {"type": "nope"}},
    }));
    let instance = Value::Object(PythonJsonObject::from_iter([(PythonString::from("v"), long_nest(325))]));
    let patterns = Mutex::new(ReCache::default());
    let model = RecursionModel::default();
    let outcome = validate_with(
        &schema,
        &instance,
        &model,
        &NoRetrieval,
        &patterns,
        HashSecret::default(),
    );
    assert_eq!(outcome_text(&outcome), "ESCAPED maximum recursion depth exceeded");
    assert!(!patterns.lock().map_err(|_| "poisoned")?.contains(WORDS));
    patterns.lock().map_err(|_| "poisoned")?.compiled(WORDS);
    let outcome = validate_with(
        &schema,
        &instance,
        &model,
        &NoRetrieval,
        &patterns,
        HashSecret::default(),
    );
    let text = outcome_text(&outcome);
    assert!(text.starts_with("RAISED Unknown type 'nope'"), "{}", clipped(&text));
    assert!(patterns.lock().map_err(|_| "poisoned")?.contains(MARGIN));
    Ok(())
}

#[test]
fn every_first_execution_site_has_a_bit_of_its_own() {
    let sites = [
        Site::CheckSchema,
        Site::IterErrors,
        Site::Descend,
        Site::Uniq,
        Site::IsValid,
        Site::UtilsIsValid,
        Site::EvaluatedItems,
        Site::EvaluatedItemsDraft2019,
        Site::EvaluatedKeysDraft2019,
        Site::UnevaluatedProperties,
        Site::UnevaluatedPropertiesDraft2019,
    ];
    let bits = sites.iter().fold(0_u16, |seen, site| {
        assert_eq!(seen & site.bit(), 0, "{site:?}");
        seen | site.bit()
    });
    assert_eq!(bits.count_ones(), 11);
}

#[test]
fn held_c_recursion_checks_add_up_and_first_executions_pay_once_a_process() -> TestResult {
    let schema = py(json!({}));
    let documents = referencing::Documents::default();
    let specifications = referencing::SPECIFICATIONS
        .as_ref()
        .map_err(|error| shown(error.clone()))?;
    let registry = Registry::new(
        specifications,
        PythonString::new(),
        Resource {
            contents: &schema,
            draft: Draft::Draft202012,
        },
        &NoRetrieval,
        &documents,
        HashSecret::default(),
    );
    let engine = Engine {
        registry,
        budget: Budget::default(),
        depth: Cell::new(0),
        format_checker: false,
        held: Cell::new(0),
        steps: RefCell::new(Vec::new()),
    };
    assert!(engine.first(Site::Uniq));
    assert!(!engine.first(Site::Uniq));
    {
        let _outer = engine.hold(3);
        let _call = engine.hold_call(Site::IsValid);
        assert_eq!(engine.c_held(), 3 + RESUMED + BUILTIN_CALL);
        {
            let _again = engine.hold_call(Site::IsValid);
            assert_eq!(engine.c_held(), 3 + 2 * RESUMED + BUILTIN_CALL);
        }
        assert_eq!(engine.c_held(), 3 + RESUMED + BUILTIN_CALL);
        let step = engine.step(Site::Descend);
        assert_eq!(engine.c_held(), 3 + 2 * RESUMED + BUILTIN_CALL);
        let nested = engine.step(Site::Descend);
        assert!(nested.flag().is_none());
        assert_eq!(engine.c_held(), 3 + 2 * RESUMED + BUILTIN_CALL);
        drop(nested);
        step.flag().ok_or("the first step holds")?.set(true);
        assert_eq!(engine.c_held(), 3 + RESUMED + BUILTIN_CALL);
        drop(step);
    }
    assert_eq!(engine.c_held(), 0);
    assert!(engine.steps.borrow().is_empty());
    assert_eq!(
        engine.budget.patterns.borrow().executed,
        Site::Uniq.bit() | Site::IsValid.bit() | Site::Descend.bit()
    );
    Ok(())
}

/// `jsonschema.validate` and `iter_errors` on schemas and instances holding lone surrogates, recorded from
/// `jsonschema` 4.26 on CPython 3.12 under several `PYTHONHASHSEED` values. A case lists one run when no seed moves
/// its result, else one run per seed.
const SURROGATES: &str = include_str!("../tests/fixtures/sdk_mcp/python_jsonschema_surrogates.json");

fn recorded_field<'v>(value: &'v Value, name: &str) -> Result<&'v Value, String> {
    value.get(name).ok_or_else(|| format!("missing {name}"))
}

/// [`outcome_text`] keeping a lone surrogate of a raised text.
fn outcome_python_text(outcome: &Outcome) -> PythonString {
    match outcome {
        Outcome::Raised(text) => PythonString::from("RAISED ")
            .code_points()
            .chain(text.code_points())
            .collect(),
        other => outcome_text(other).into(),
    }
}

/// `jsonschema.validate` and `iter_errors` on schemas whose URIs, references and anchors hold lone surrogates,
/// recorded as [`SURROGATES`] is, with `urlopen` answering each of the fixture's `documents` and raising `URLError`
/// for any other URI.
const URI_SURROGATES: &str = include_str!("../tests/fixtures/sdk_mcp/python_jsonschema_uri_surrogates.json");

/// Answers each URI a fixture records a document for, and fails any other, as `urlopen` fails where no server
/// answers.
struct RecordedDocuments(HashMap<String, Vec<u8>>);

impl RemoteSchemaRetriever for RecordedDocuments {
    fn open(&self, uri: &str) -> Result<Box<dyn Read + '_>, RemoteSchemaError> {
        match self.0.get(uri) {
            Some(body) => Ok(Box::new(body.as_slice())),
            None => Err(RemoteSchemaError::new("URLError", uri)),
        }
    }
}

#[test]
fn lone_surrogates_reach_messages_paths_and_raised_texts_as_jsonschema_gives_them() -> TestResult {
    let fixture = python_json_loads(SURROGATES)?;
    assert_eq!(replay_surrogate_cases(&fixture, &NoRetrieval)?, (34, 8));
    Ok(())
}

#[test]
fn lone_surrogates_in_uris_references_and_anchors_resolve_as_referencing_resolves_them() -> TestResult {
    let fixture = python_json_loads(URI_SURROGATES)?;
    let documents = recorded_field(&fixture, "documents")?
        .as_object()
        .ok_or("documents is not an object")?
        .iter()
        .map(|(uri, body)| -> Result<(String, Vec<u8>), String> {
            let uri = uri.as_str().ok_or("a document URI holds a lone surrogate")?;
            let body = body.as_str().ok_or("a document is not a str")?;
            Ok((uri.to_owned(), body.as_bytes().to_vec()))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    assert_eq!(documents.len(), 4);
    assert_eq!(
        replay_surrogate_cases(&fixture, &RecordedDocuments(documents))?,
        (36, 8)
    );
    Ok(())
}

/// Replays each case of a surrogate fixture under each of its seeds, retrieving through `retriever`: the count of
/// cases and of seeds.
fn replay_surrogate_cases(
    fixture: &Value,
    retriever: &dyn RemoteSchemaRetriever,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let seeds = recorded_field(fixture, "seeds")?
        .as_array()
        .ok_or("seeds is not a list")?;
    let cases = recorded_field(fixture, "cases")?
        .as_array()
        .ok_or("cases is not a list")?;
    for case in cases {
        let schema = recorded_field(case, "schema")?;
        let instance = recorded_field(case, "instance")?;
        let runs = recorded_field(case, "runs")?.as_array().ok_or("runs is not a list")?;
        let shown = python_json_dumps(schema);
        for (at, seed) in seeds.iter().enumerate() {
            let seed = seed
                .as_number()
                .and_then(|number| number.as_u64())
                .ok_or("a seed is not a number")?;
            let secret = HashSecret::from_seed(u32::try_from(seed)?);
            let run = runs.get(at).or_else(|| runs.first()).ok_or("a case has no run")?;
            let outcome = validate_retrieving(schema, instance, retriever, &Mutex::default(), secret);
            assert_eq!(
                Some(&outcome_python_text(&outcome)),
                recorded_field(run, "outcome")?.as_python_str(),
                "{shown} seed {seed}"
            );
            // `validator_for(schema)` raises inside the recorded `try`, so a `$schema` it rejects is the raised text.
            let (errors, raised) = match drafts::validator_for(schema, Draft::Draft202012) {
                Ok(draft) => {
                    let mut budget = Budget {
                        secret,
                        ..Budget::default()
                    };
                    iter_errors_spending(draft, schema, instance, &mut budget, retriever)
                }
                Err(error) => (Vec::new(), Some(error)),
            };
            assert_eq!(
                &Value::Array(errors.iter().map(encode).collect()),
                recorded_field(run, "errors")?,
                "{shown} seed {seed}"
            );
            assert_eq!(
                &raised.map_or(Value::Null, Value::String),
                recorded_field(run, "raised")?,
                "{shown} seed {seed}"
            );
        }
    }
    Ok((cases.len(), seeds.len()))
}

/// Outcomes `jsonschema` 4.26 gives for the draft 3 keywords and the `unevaluated*` walks, each through
/// `best_match(cls(schema).iter_errors(instance))`, or `str(exc)` for what it raises.
#[test]
fn draft3_keywords_and_unevaluated_walks_match_jsonschema() {
    let unevaluated_items = "Unevaluated items are not allowed (1 was unexpected)";
    let unevaluated_a = "Unevaluated properties are not allowed ('a' was unexpected)";
    let unevaluated_b = "Unevaluated properties are not allowed ('b' was unexpected)";
    let invalid = |message: &str| Outcome::Invalid(message.to_owned());
    let raised = |text: &str| Outcome::Raised(text.into());
    let cases = [
        (
            Draft::Draft3,
            json!({"disallow": "string"}),
            json!("a"),
            invalid("'string' is disallowed for 'a'"),
        ),
        (
            Draft::Draft3,
            json!({"disallow": ["string", {"type": "integer"}]}),
            json!(1),
            invalid("{'type': 'integer'} is disallowed for 1"),
        ),
        (
            Draft::Draft3,
            json!({"extends": [{"type": "string"}, {"minimum": 3}]}),
            json!(1),
            invalid("1 is not of type 'string'"),
        ),
        (
            Draft::Draft3,
            json!({"extends": [{"type": "string"}]}),
            json!("a"),
            Outcome::Valid,
        ),
        // `not` asks `is_valid`, whose first error stops the draft 3 keyword a `$schema` evolves into.
        (
            Draft::Draft202012,
            json!({"not": {"$schema": "http://json-schema.org/draft-03/schema#", "disallow": "string"}}),
            json!("a"),
            Outcome::Valid,
        ),
        (
            Draft::Draft202012,
            json!({"not": {"$schema": "http://json-schema.org/draft-03/schema#", "extends": [{"type": "string"}]}}),
            json!(1),
            Outcome::Valid,
        ),
        (
            Draft::Draft202012,
            json!({"allOf": [true], "unevaluatedItems": false}),
            json!([1]),
            invalid(unevaluated_items),
        ),
        (
            Draft::Draft202012,
            json!({"allOf": [true], "unevaluatedProperties": false}),
            json!({"a": 1}),
            invalid(unevaluated_a),
        ),
        (
            Draft::Draft201909,
            json!({"allOf": [true], "unevaluatedItems": false}),
            json!([1]),
            invalid(unevaluated_items),
        ),
        (
            Draft::Draft201909,
            json!({"allOf": [true], "unevaluatedProperties": false}),
            json!({"a": 1}),
            invalid(unevaluated_a),
        ),
        (
            Draft::Draft202012,
            json!({"unevaluatedItems": false, "allOf": [{"$ref": "#/nope"}]}),
            json!([1]),
            raised(
                "PointerToNowhere: '/nope' does not exist within {'unevaluatedItems': False, 'allOf': [{'$ref': \
                 '#/nope'}]}",
            ),
        ),
        (
            Draft::Draft201909,
            json!({"unevaluatedItems": false, "allOf": [{"$ref": "#/nope"}]}),
            json!([1]),
            raised(
                "PointerToNowhere: '/nope' does not exist within {'unevaluatedItems': False, 'allOf': [{'$ref': \
                 '#/nope'}]}",
            ),
        ),
        (
            Draft::Draft201909,
            json!({"unevaluatedProperties": false, "allOf": [{"$ref": "#/nope"}]}),
            json!({"a": 1}),
            raised(
                "PointerToNowhere: '/nope' does not exist within {'unevaluatedProperties': False, 'allOf': [{'$ref': \
                 '#/nope'}]}",
            ),
        ),
        (
            Draft::Draft202012,
            json!({"unevaluatedItems": false, "$ref": "#/nope"}),
            json!([1]),
            raised("'/nope' does not exist within {'unevaluatedItems': False, '$ref': '#/nope'}"),
        ),
        (
            Draft::Draft202012,
            json!({"x": "ab", "$ref": "#/x/0/0"}),
            json!(1),
            raised("'str' object has no attribute 'items'"),
        ),
        (
            Draft::Draft202012,
            json!({"x": "ab", "$ref": "#/x/0/1"}),
            json!(1),
            raised("PointerToNowhere: '/x/0/1' does not exist within {'x': 'ab', '$ref': '#/x/0/1'}"),
        ),
        (
            Draft::Draft201909,
            json!({"items": {"unevaluatedItems": false, "allOf": [{"$recursiveRef": "#"}]}}),
            json!([[1]]),
            Outcome::Valid,
        ),
        (
            Draft::Draft201909,
            json!({"items": [true], "additionalItems": {"unevaluatedItems": false, "$recursiveRef": "#"}}),
            json!([0, [1, 2]]),
            Outcome::Valid,
        ),
        (
            Draft::Draft201909,
            json!({
                "properties": {"a": true},
                "additionalProperties": {"unevaluatedProperties": false, "allOf": [{"$recursiveRef": "#"}]},
            }),
            json!({"x": {"a": 1, "b": 2}}),
            invalid(unevaluated_b),
        ),
        (
            Draft::Draft201909,
            json!({
                "properties": {"a": true},
                "additionalProperties": {"unevaluatedProperties": false, "$recursiveRef": "#"},
            }),
            json!({"x": {"a": 1, "b": 2}}),
            invalid(unevaluated_b),
        ),
        (
            Draft::Draft201909,
            json!({
                "properties": {"a": true},
                "additionalProperties": {"unevaluatedProperties": false, "$recursiveRef": "#"},
            }),
            json!({"x": {"a": 1}}),
            Outcome::Valid,
        ),
        (
            Draft::Draft202012,
            json!({"if": true, "unevaluatedProperties": false}),
            json!({"a": 1}),
            invalid(unevaluated_a),
        ),
        (
            Draft::Draft201909,
            json!({"additionalProperties": 5, "unevaluatedProperties": false}),
            json!({"a": 1}),
            invalid(unevaluated_a),
        ),
    ];
    for (draft, schema, instance, expected) in cases {
        let shown = schema.to_string();
        assert_eq!(
            best_match(draft, &py(schema), &py(instance)),
            expected,
            "{} {shown}",
            class_name(draft)
        );
    }
}

/// A retrieved document whose `$id` is no string fails the anchor lookup the way `Resource.id()` fails in
/// `referencing`, as `jsonschema` 4.26 raises it with the document served over HTTP.
#[test]
fn an_anchor_in_a_retrieved_document_with_a_non_string_id_raises_its_attribute_error() {
    let retriever = RecordedDocuments(HashMap::from([("http://h/s".to_owned(), br#"{"$id": 5}"#.to_vec())]));
    assert_eq!(
        best_match_retrieving(
            Draft::Draft202012,
            &py(json!({"$ref": "http://h/s#a"})),
            &py(json!(1)),
            &retriever
        ),
        Outcome::Raised("'int' object has no attribute 'rstrip'".into())
    );
    assert_eq!(
        best_match_retrieving(
            Draft::Draft202012,
            &py(json!({"$ref": "http://h/s"})),
            &py(json!(1)),
            &retriever
        ),
        Outcome::Valid
    );
}
