use super::*;

#[test]
fn checked_budgets_reject_nonfinite_and_negative_values() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert!(UsdBudget::checked(value).is_err());
        assert_eq!(UsdBudget::new(value).get().to_bits(), value.to_bits());
    }
    assert!(UsdBudget::checked(0.0).is_ok());
}

#[test]
fn typescript_text_matches_javascript_string_conversion() {
    for (value, text) in [
        (0.25, "0.25"),
        (1.0, "1"),
        (0.0, "0"),
        (-0.0, "0"),
        (-1.5, "-1.5"),
        (1e-7, "1e-7"),
        (0.000001, "0.000001"),
        (1e21, "1e+21"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
    ] {
        assert_eq!(UsdBudget::new(value).typescript_text(), text, "{value}");
    }
}

#[test]
fn python_text_matches_cpython_float_repr() {
    for (value, text) in [
        (0.25, "0.25"),
        (1.0, "1.0"),
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (-1.5, "-1.5"),
        (0.0001, "0.0001"),
        (1e-5, "1e-05"),
        (1.5e-7, "1.5e-07"),
        (1e15, "1000000000000000.0"),
        (1e16, "1e+16"),
        (1.25e100, "1.25e+100"),
        (123.456, "123.456"),
        (f64::NAN, "nan"),
        (f64::INFINITY, "inf"),
        (f64::NEG_INFINITY, "-inf"),
    ] {
        assert_eq!(UsdBudget::new(value).python_text(), text, "{value}");
    }
}
