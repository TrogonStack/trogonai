//! `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` parsed by the real upstream code, captured into a fixture whose `note`
//! says how. Each expected value is the applied ceiling in milliseconds, with null for no limit.

use serde_json::Value;
use std::{collections::HashMap, error::Error as StdError, time::Duration};
use trogon_claude_agent_sdk::RunEndTimeout;

type TestResult = Result<(), Box<dyn StdError>>;

const FIXTURE: &str = include_str!("fixtures/run_end_ceiling/ceiling.json");
const DEFAULT: Option<Duration> = Some(Duration::from_millis(600_000));

fn fixture() -> Result<Value, serde_json::Error> {
    serde_json::from_str(FIXTURE)
}

fn applied(value: &Value) -> Option<Duration> {
    value.as_u64().map(Duration::from_millis)
}

fn table(dialect: RunEndTimeout, name: &str) -> TestResult {
    let fixture = fixture()?;
    let rows = fixture["table"].as_array().ok_or("the table is missing")?;
    assert!(rows.len() >= 100, "the table has {} rows", rows.len());
    for row in rows {
        let input = row["input"].as_str();
        assert_eq!(
            dialect.for_variable(input),
            applied(&row[name]),
            "{name} input {input:?}"
        );
    }
    Ok(())
}

/// Every Unicode scalar value except NUL, made into a string by `make`, against the entries that differ from the
/// default.
fn sweep(dialect: RunEndTimeout, entries: &Value, make: impl Fn(char) -> String, label: &str) -> TestResult {
    let entries = entries.as_array().ok_or("a sweep is missing")?;
    assert!(!entries.is_empty(), "{label} has no entries");
    let expected: HashMap<u64, Option<Duration>> = entries
        .iter()
        .map(|entry| (entry[0].as_u64().unwrap_or_default(), applied(&entry[1])))
        .collect();
    assert_eq!(expected.len(), entries.len(), "{label} repeats a code point");
    for code in 1..=0x10_ffff_u32 {
        let Some(character) = char::from_u32(code) else {
            continue;
        };
        let want = expected.get(&u64::from(code)).copied().unwrap_or(DEFAULT);
        assert_eq!(
            dialect.for_variable(Some(&make(character))),
            want,
            "{label} U+{code:04X}"
        );
    }
    Ok(())
}

#[test]
fn typescript_matches_the_real_sdk_on_the_table() -> TestResult {
    table(RunEndTimeout::TypeScript, "typescript")
}

#[test]
fn python_matches_the_real_package_on_the_table() -> TestResult {
    table(RunEndTimeout::Python, "python")
}

#[test]
fn typescript_matches_the_real_sdk_on_every_code_point() -> TestResult {
    let fixture = fixture()?;
    let typescript = &fixture["codepoints"]["typescript"];
    sweep(
        RunEndTimeout::TypeScript,
        &typescript["solo"],
        String::from,
        "typescript alone",
    )?;
    sweep(
        RunEndTimeout::TypeScript,
        &typescript["leading_then_7"],
        |character| format!("{character}7"),
        "typescript before 7",
    )
}

#[test]
fn python_matches_the_real_package_on_every_code_point() -> TestResult {
    let fixture = fixture()?;
    let python = &fixture["codepoints"]["python"];
    sweep(RunEndTimeout::Python, &python["solo"], String::from, "python alone")?;
    sweep(
        RunEndTimeout::Python,
        &python["leading_then_1"],
        |character| format!("{character}1"),
        "python before 1",
    )?;
    sweep(
        RunEndTimeout::Python,
        &python["1_then_trailing"],
        |character| format!("1{character}"),
        "python after 1",
    )
}

/// An explicit wait is opt-in and ignores the variable, zero included.
#[test]
fn a_fixed_wait_ignores_the_text() {
    let fixed = RunEndTimeout::Fixed(Duration::from_millis(5).into());
    assert_eq!(fixed.for_variable(Some("0")), Some(Duration::from_millis(5)));
    assert_eq!(fixed.for_variable(None), Some(Duration::from_millis(5)));
}
