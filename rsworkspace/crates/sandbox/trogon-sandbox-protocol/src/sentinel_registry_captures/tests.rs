use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_capture_is_registered_under_a_derived_name() -> TestResult {
    let binding = SecretName::new("token")?;
    assert_eq!(capture_entry_name(&binding, 2)?.as_str(), "token.capture-2");
    Ok(())
}

#[test]
fn a_refused_derived_name_reports_the_binding_and_the_capture() -> TestResult {
    let binding = SecretName::new("token")?;
    let error = CaptureMaskError::entry_name(&binding, 3);
    assert_eq!(
        error,
        CaptureMaskError::CaptureEntryName {
            binding: binding.clone(),
            index: 3
        }
    );
    assert_eq!(
        error.to_string(),
        "the name for capture 3 of 'token' is not a valid secret name"
    );
    Ok(())
}
