use super::*;

#[test]
fn says_which_bound_the_limit_missed() {
    assert_eq!(AdmissionLimitError::Zero.to_string(), "admission limit must be nonzero");
    assert_eq!(
        AdmissionLimitError::TooLarge.to_string(),
        "admission limit exceeds the supported permit range"
    );
}
