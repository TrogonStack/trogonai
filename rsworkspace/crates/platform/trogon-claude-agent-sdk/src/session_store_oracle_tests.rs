//! Fixtures written by `.config/mise/lib/claude-sdk-freshness/session_store_oracle.mjs`, which runs the pinned 0.3.323
//! bundle's own helpers on the same inputs.

use serde_json::Value;

use crate::{
    session_fork_ts::js_stringify,
    session_mutations::{SessionMutationError, sanitize_unicode_ts},
    session_project_key::project_name_with,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn cases(fixture: &str) -> Result<Vec<Value>, serde_json::Error> {
    serde_json::from_str(fixture)
}

fn text<'a>(case: &'a Value, name: &str) -> Result<&'a str, String> {
    case.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("fixture case without `{name}`: {case}"))
}

#[test]
fn sanitize_unicode_ts_matches_the_bundle() -> TestResult {
    let cases = cases(include_str!("../tests/fixtures/oracle_ts/sanitize_unicode.json"))?;
    assert!(cases.len() > 20);
    for case in cases {
        let input = text(&case, "input")?;
        match (case.get("output").and_then(Value::as_str), sanitize_unicode_ts(input)) {
            (Some(expected), Ok(actual)) => assert_eq!(actual, expected, "input {input:?}"),
            (None, Err(SessionMutationError::UnicodeSanitization(_))) => {}
            (expected, actual) => {
                return Err(format!("input {input:?}: expected {expected:?}, got {actual:?}").into());
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn lexical_resolve_matches_node_path_resolve() -> TestResult {
    let cases = cases(include_str!("../tests/fixtures/oracle_ts/lexical_resolve.json"))?;
    assert!(cases.len() > 10);
    for case in cases {
        let input = text(&case, "input")?;
        let resolved = crate::session_project_key::lexical_resolve(std::path::Path::new(input));
        assert_eq!(resolved.to_str(), Some(text(&case, "output")?), "input {input:?}");
    }
    Ok(())
}

#[test]
fn project_name_with_matches_the_bundle() -> TestResult {
    let cases = cases(include_str!("../tests/fixtures/oracle_ts/project_name.json"))?;
    assert!(cases.len() > 50);
    for case in cases {
        let directory = text(&case, "directory")?;
        let config_dir = case.get("config_dir").and_then(Value::as_str);
        let name_override = case.get("name_override").and_then(Value::as_str);
        assert_eq!(
            project_name_with(directory, config_dir, name_override),
            text(&case, "output")?,
            "case {case}"
        );
    }
    Ok(())
}

#[test]
fn js_key_order_matches_json_stringify() -> TestResult {
    let cases = cases(include_str!("../tests/fixtures/oracle_ts/js_key_order.json"))?;
    assert!(cases.len() > 10);
    for case in cases {
        let input = text(&case, "input")?;
        let parsed: Value = serde_json::from_str(input)?;
        assert_eq!(js_stringify(&parsed), text(&case, "output")?, "input {input}");
    }
    Ok(())
}
