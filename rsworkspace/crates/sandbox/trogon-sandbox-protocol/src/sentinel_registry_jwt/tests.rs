use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_claim_is_registered_under_a_derived_name() -> TestResult {
    let binding = SecretName::new("token")?;
    assert_eq!(claim_entry_name(&binding, 1)?.as_str(), "token.claim-1");
    Ok(())
}

#[test]
fn a_refused_derived_name_reports_the_binding_and_the_claim() -> TestResult {
    let binding = SecretName::new("token")?;
    let error = JwtMaskError::entry_name(&binding, 4);
    assert_eq!(
        error,
        JwtMaskError::ClaimEntryName {
            binding: binding.clone(),
            index: 4
        }
    );
    assert_eq!(
        error.to_string(),
        "the name for claim 4 of 'token' is not a valid secret name"
    );
    Ok(())
}
