use super::fresh_terminal_id;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn fresh_ids_are_valid_and_distinct() -> TestResult {
    let a = fresh_terminal_id()?;
    let b = fresh_terminal_id()?;
    assert_ne!(a, b);
    assert!(a.as_str().starts_with("mcp-"));
    Ok(())
}
