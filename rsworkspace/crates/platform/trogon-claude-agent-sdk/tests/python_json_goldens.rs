//! Checks `python_json_loads` and `python_json_dumps` against what Python's `json.loads` and `json.dumps` did with
//! the same text in CPython (`.config/mise/lib/claude-sdk-freshness/python_json_goldens.py` writes the goldens).

use serde_json::Value;
use trogon_claude_agent_sdk::{
    ExactIntegerError, PythonJsonError, PythonJsonValue, PythonNumber, python_json_dumps, python_json_loads,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/python_json/goldens.json");

fn entries(section: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut goldens: Value = serde_json::from_str(GOLDENS)?;
    match goldens.get_mut(section).map(Value::take) {
        Some(Value::Array(entries)) => Ok(entries),
        _ => Err(format!("goldens have no {section} list").into()),
    }
}

fn pairs(section: &str) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    entries(section)?
        .into_iter()
        .map(|entry| match entry.as_array().map(Vec::as_slice) {
            Some([Value::String(input), Value::String(output)]) => Ok((input.clone(), output.clone())),
            _ => Err(format!("malformed {section} entry {entry}").into()),
        })
        .collect()
}

#[test]
fn every_double_prints_like_json_dumps() -> TestResult {
    let numbers = pairs("numbers")?;
    assert!(numbers.len() > 5000, "the corpus shrank to {} numbers", numbers.len());
    let mut mismatches = Vec::new();
    for (bits, expected) in numbers {
        let double = f64::from_bits(u64::from_str_radix(&bits, 16)?);
        let printed = python_json_dumps(&PythonJsonValue::Number(PythonNumber::Float(double)));
        if printed != expected {
            mismatches.push(format!("{bits}: printed {printed}, json.dumps printed {expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}

#[test]
fn every_document_prints_like_json_dumps_after_json_loads() -> TestResult {
    let documents = pairs("documents")?;
    assert!(
        documents.len() > 400,
        "the corpus shrank to {} documents",
        documents.len()
    );
    let mut mismatches = Vec::new();
    for (source, expected) in documents {
        match python_json_loads(&source) {
            Ok(value) => {
                let printed = python_json_dumps(&value);
                if printed != expected {
                    mismatches.push(format!("{source}: printed {printed}, json.dumps printed {expected}"));
                }
            }
            Err(error) => mismatches.push(format!("{source}: refused with {error}, json.loads read it")),
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}

/// The digit limit and digit count CPython's `ValueError` names, read back from its recorded message.
fn digit_counts(message: &str) -> Option<(usize, usize)> {
    let (_, after_limit) = message.split_once('(')?;
    let (limit, rest) = after_limit.split_once(' ')?;
    let (_, after_has) = rest.split_once("value has ")?;
    let (digits, _) = after_has.split_once(' ')?;
    Some((limit.parse().ok()?, digits.parse().ok()?))
}

#[test]
fn every_refused_text_is_refused_like_json_loads_refuses_it() -> TestResult {
    let refused = entries("refused")?;
    assert!(refused.len() > 20, "the corpus shrank to {} refusals", refused.len());
    let mut mismatches = Vec::new();
    for entry in refused {
        let Some([Value::String(source), Value::String(kind), message]) = entry.as_array().map(Vec::as_slice) else {
            return Err(format!("malformed refused entry {entry}").into());
        };
        let outcome = python_json_loads(source);
        let matched = match (kind.as_str(), &outcome) {
            ("syntax", Err(PythonJsonError::Syntax(_))) | ("bom", Err(PythonJsonError::ByteOrderMark)) => true,
            ("integer", Err(PythonJsonError::Integer(ExactIntegerError::TooManyDigits { limit, digits }))) => {
                message.as_str().and_then(digit_counts) == Some((*limit, *digits))
            }
            _ => false,
        };
        if !matched {
            let shown: String = source.chars().take(40).collect();
            mismatches.push(format!(
                "{shown:?}: read as {outcome:?}, json.loads refused it with {kind} {message}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}
