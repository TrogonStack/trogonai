use super::*;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `fnmatch.fnmatchcase(name, pattern)` of CPython 3.12.16 for handpicked and random names and patterns, with the
/// `_scproxy` cases the same recorder took.
const PYTHON_SYSCONF: &str = include_str!("../tests/fixtures/sdk_mcp/python_sysconf.json");

/// A recorded text: a string, or its code points when it holds a lone surrogate.
fn recorded_text(value: &Value) -> Result<PythonString, Box<dyn std::error::Error>> {
    if let Some(text) = value.as_str() {
        return Ok(PythonString::from(text));
    }
    value["code_points"]
        .as_array()
        .ok_or_else(|| format!("not a recorded text: {value}"))?
        .iter()
        .map(|code| {
            code.as_u64()
                .and_then(|code| u32::try_from(code).ok())
                .and_then(|code| PythonCodePoint::try_from(code).ok())
                .ok_or_else(|| format!("not a code point: {code}").into())
        })
        .collect()
}

fn translated(pattern: &str) -> String {
    let pattern: Vec<u32> = pattern.chars().map(u32::from).collect();
    translate(&pattern).into_iter().filter_map(char::from_u32).collect()
}

#[test]
fn fnmatchcase_matches_the_recorded_pairs() -> TestResult {
    let golden: Value = serde_json::from_str(PYTHON_SYSCONF)?;
    let pairs = golden["fnmatch"].as_array().ok_or("the golden has fnmatch pairs")?;
    assert_eq!(pairs.len(), 4040);
    let mut matched = 0;
    for pair in pairs {
        let name = recorded_text(&pair["name"])?;
        let pattern = recorded_text(&pair["pattern"])?;
        let expected = pair["result"].as_bool().ok_or("each pair has a result")?;
        assert_eq!(fnmatchcase(&name, &pattern)?, expected, "{name:?} {pattern:?}");
        matched += usize::from(expected);
    }
    assert_eq!(matched, 171);
    Ok(())
}

#[test]
fn translate_writes_what_cpython_3_12_writes() {
    assert_eq!(translated("*.local"), r"(?s:.*\.local)\Z");
    assert_eq!(translated("a*b*c"), r"(?s:a(?>.*?b).*c)\Z");
    assert_eq!(translated("**"), r"(?s:.*)\Z");
    assert_eq!(translated("[!a-c]?"), r"(?s:[^a-c].)\Z");
    assert_eq!(translated("[z-a]"), r"(?s:(?!))\Z");
    assert_eq!(translated("[!]"), r"(?s:\[!\])\Z");
    assert_eq!(translated("[a-]"), r"(?s:[a\-])\Z");
    assert_eq!(translated("[^&]"), r"(?s:[\^\&])\Z");
    assert_eq!(translated("[\\]"), r"(?s:[\\])\Z");
    assert_eq!(translated("[a"), r"(?s:\[a)\Z");
}
