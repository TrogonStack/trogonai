//! Checks `js_stringify` against what `JSON.stringify` printed for the same values in Node
//! (`.config/mise/lib/claude-sdk-freshness/js_stringify_goldens.mjs` writes the goldens).

use serde_json::{Number, Value};
use trogon_claude_agent_sdk::js_stringify;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("fixtures/js_stringify/goldens.json");

fn pairs(section: &str) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entries = goldens
        .get(section)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("goldens have no {section} list"))?;
    entries
        .iter()
        .map(|entry| match entry.as_array().map(Vec::as_slice) {
            Some([Value::String(input), Value::String(output)]) => Ok((input.clone(), output.clone())),
            _ => Err(format!("malformed {section} entry {entry}").into()),
        })
        .collect()
}

#[test]
fn every_double_prints_like_json_stringify() -> TestResult {
    let numbers = pairs("numbers")?;
    assert!(numbers.len() > 5000, "the corpus shrank to {} numbers", numbers.len());
    let mut mismatches = Vec::new();
    for (bits, expected) in numbers {
        let double = f64::from_bits(u64::from_str_radix(&bits, 16)?);
        let number = Number::from_f64(double).ok_or_else(|| format!("{bits} is not finite"))?;
        let printed = js_stringify(&Value::Number(number));
        if printed != expected {
            mismatches.push(format!("{bits}: printed {printed}, JSON.stringify printed {expected}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}

#[test]
fn every_document_prints_like_json_stringify_after_json_parse() -> TestResult {
    let documents = pairs("documents")?;
    assert!(
        documents.len() > 400,
        "the corpus shrank to {} documents",
        documents.len()
    );
    let mut mismatches = Vec::new();
    for (source, expected) in documents {
        let value: Value = serde_json::from_str(&source)?;
        let printed = js_stringify(&value);
        if printed != expected {
            mismatches.push(format!(
                "{source}: printed {printed}, JSON.stringify printed {expected}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    Ok(())
}
