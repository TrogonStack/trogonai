use super::*;
use crate::test_fixtures::TestResult;

#[test]
fn a_generated_identity_is_a_valid_label_value() -> TestResult {
    let generated = WorkerIdentity::generate();
    assert_eq!(generated.as_str().parse::<WorkerIdentity>()?, generated);
    assert_ne!(generated, WorkerIdentity::generate());
    Ok(())
}

#[test]
fn only_label_values_parse() {
    for valid in ["worker-0", "a", "a.b_c-d"] {
        assert!(valid.parse::<WorkerIdentity>().is_ok(), "{valid}");
    }
    for invalid in ["", "-worker", "worker-", "wor ker", &"a".repeat(64)] {
        assert!(invalid.parse::<WorkerIdentity>().is_err(), "{invalid}");
    }
}
