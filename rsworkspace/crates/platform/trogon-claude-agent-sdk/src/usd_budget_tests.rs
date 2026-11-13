use super::*;
#[test]
fn budget_rejects_nonfinite_and_negative_values() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert!(UsdBudget::new(value).is_err());
    }
    assert!(UsdBudget::new(0.0).is_ok());
}
