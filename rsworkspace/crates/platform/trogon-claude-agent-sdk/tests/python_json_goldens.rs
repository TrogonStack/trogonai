//! Checks `python_json_loads` and `python_json_dumps` against what Python's `json.loads` and `json.dumps` did with
//! the same text in CPython (`.config/mise/lib/claude-sdk-freshness/python_json_goldens.py` writes the goldens).

use serde::de::DeserializeOwned;
use serde_json::Value;
use trogon_claude_agent_sdk::{PythonJsonError, PythonJsonValue, PythonNumber, python_json_dumps, python_json_loads};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/python_json/goldens.json");

fn section<T: DeserializeOwned>(name: &str) -> Result<T, Box<dyn std::error::Error>> {
    let mut goldens: Value = serde_json::from_str(GOLDENS)?;
    Ok(serde_json::from_value(
        goldens.get_mut(name).map(Value::take).unwrap_or_default(),
    )?)
}

/// What `json.loads` followed by `json.dumps` gives for `source`, in the goldens' terms: `ok` with the printed
/// value, or the kind of exception with its message.
fn outcome(source: &str) -> (String, String, String) {
    let (kind, payload) = match python_json_loads(source) {
        Ok(value) => ("ok", python_json_dumps(&value)),
        Err(error @ PythonJsonError::Decode(_)) => ("decode", error.to_string()),
        Err(error @ PythonJsonError::Integer(_)) => ("integer", error.to_string()),
        Err(error @ PythonJsonError::Recursion(_)) => ("recursion", error.to_string()),
    };
    (source.to_owned(), kind.to_owned(), payload)
}

#[test]
fn every_double_prints_like_json_dumps() -> TestResult {
    let numbers: Vec<(String, String)> = section("numbers")?;
    assert!(numbers.len() > 5000, "the corpus shrank to {} numbers", numbers.len());
    let printed = numbers
        .iter()
        .map(|(bits, _)| {
            u64::from_str_radix(bits, 16).map(|pattern| {
                let double = PythonNumber::Float(f64::from_bits(pattern));
                (bits.clone(), python_json_dumps(&PythonJsonValue::Number(double)))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(printed, numbers);
    Ok(())
}

#[test]
fn every_document_prints_like_json_dumps_after_json_loads() -> TestResult {
    let documents: Vec<(String, String)> = section("documents")?;
    assert!(
        documents.len() > 400,
        "the corpus shrank to {} documents",
        documents.len()
    );
    let read: Vec<_> = documents.iter().map(|(source, _)| outcome(source)).collect();
    let expected: Vec<_> = documents
        .into_iter()
        .map(|(source, printed)| (source, "ok".to_owned(), printed))
        .collect();
    assert_eq!(read, expected);
    Ok(())
}

#[test]
fn every_refused_text_is_refused_like_json_loads_refuses_it() -> TestResult {
    let refused: Vec<(String, String, String)> = section("refused")?;
    assert!(refused.len() > 20, "the corpus shrank to {} refusals", refused.len());
    let read: Vec<_> = refused.iter().map(|(source, _, _)| outcome(source)).collect();
    assert_eq!(read, refused);
    Ok(())
}

#[test]
fn every_fuzzed_text_reads_like_json_loads_reads_it() -> TestResult {
    let fuzzed: Vec<(String, String, String)> = section("fuzzed")?;
    assert!(fuzzed.len() > 3000, "the corpus shrank to {} texts", fuzzed.len());
    let read: Vec<_> = fuzzed.iter().map(|(source, _, _)| outcome(source)).collect();
    assert_eq!(read, fuzzed);
    Ok(())
}

#[test]
fn nesting_past_the_limit_raises_the_recursion_error_json_loads_raises() -> TestResult {
    let recursion: Vec<(String, usize, String)> = section("recursion")?;
    assert_eq!(recursion.len(), 4);
    let read: Vec<_> = recursion
        .iter()
        .map(|(opening, count, _)| {
            let (_, kind, message) = outcome(&opening.repeat(*count));
            (opening.clone(), *count, format!("{kind} {message}"))
        })
        .collect();
    let expected: Vec<_> = recursion
        .into_iter()
        .map(|(opening, count, message)| (opening, count, format!("recursion {message}")))
        .collect();
    assert_eq!(read, expected);
    Ok(())
}
