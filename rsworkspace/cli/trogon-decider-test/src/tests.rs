use super::*;

fn error_expectation(yaml: &str) -> String {
    match serde_yaml::from_str::<Then>(yaml) {
        Ok(Then::Error { error }) => error.expected().unwrap_or_default(),
        _ => String::new(),
    }
}

#[test]
fn then_error_accepts_structured_code() {
    assert_eq!(error_expectation("error:\n  code: already-exists\n"), "already-exists");
}

#[test]
fn then_error_accepts_plain_string() {
    assert_eq!(error_expectation("error: already-exists\n"), "already-exists");
}
