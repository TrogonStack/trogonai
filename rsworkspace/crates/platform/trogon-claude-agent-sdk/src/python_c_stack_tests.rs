use serde_json::Value;

use super::PythonCStack;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("../tests/fixtures/settings_option/python_settings.json");

#[test]
fn each_caller_reads_as_deep_as_python_does_there() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let expected: Vec<(String, u64)> = goldens["depth"]
        .as_array()
        .ok_or("the goldens have no depth entries")?
        .iter()
        .map(|entry| -> Result<(String, u64), &'static str> {
            let caller = entry["caller"].as_str().ok_or("a depth entry has no caller")?;
            let loads = entry["loads"].as_u64().ok_or("a depth entry has no loads")?;
            Ok((caller.to_owned(), loads))
        })
        .collect::<Result<_, &'static str>>()?;
    let actual: Vec<(String, u64)> = [
        ("module", PythonCStack::MODULE),
        ("client_connect", PythonCStack::CLIENT_CONNECT),
        ("query", PythonCStack::QUERY),
    ]
    .into_iter()
    .map(|(caller, stack)| -> Result<(String, u64), std::num::TryFromIntError> {
        Ok((caller.to_owned(), u64::try_from(stack.json_depth())?))
    })
    .collect::<Result<_, std::num::TryFromIntError>>()?;
    assert_eq!(actual, expected);
    Ok(())
}
