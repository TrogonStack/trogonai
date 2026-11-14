use std::collections::HashMap;

use super::*;

/// What CPython's `str(float)` printed for each double of a corpus, as `[bits, text]` pairs per section
/// (`.config/mise/lib/claude-sdk-freshness/usd_budget_goldens.py` writes it).
const GOLDENS: &str = include_str!("../tests/fixtures/usd_budget/goldens.json");

/// The pairs of one section of the goldens, next to the pairs `python_text` makes of the same doubles.
type Pairs = Vec<(String, String)>;
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn printed_and_expected(section: &str) -> Result<(Pairs, Pairs), Box<dyn std::error::Error>> {
    let mut goldens: HashMap<String, Pairs> = serde_json::from_str(GOLDENS)?;
    let expected = goldens.remove(section).unwrap_or_default();
    let printed = expected
        .iter()
        .map(|(bits, _)| {
            let value = f64::from_bits(u64::from_str_radix(bits, 16)?);
            Ok((bits.clone(), UsdBudget::new(value).python_text()))
        })
        .collect::<Result<Pairs, std::num::ParseIntError>>()?;
    Ok((printed, expected))
}

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

#[test]
fn python_text_keeps_the_even_digit_on_a_tie_like_cpython() -> TestResult {
    let (printed, expected) = printed_and_expected("ties")?;
    assert!(expected.len() > 500, "the corpus shrank to {} ties", expected.len());
    let two_to_the_minus_25 = (
        format!("{:016x}", 2f64.powi(-25).to_bits()),
        "2.9802322387695312e-08".to_owned(),
    );
    assert!(expected.contains(&two_to_the_minus_25), "the corpus lost 2**-25");
    assert_eq!(printed, expected);
    Ok(())
}

#[test]
fn python_text_prints_every_double_like_cpython_str() -> TestResult {
    let (printed, expected) = printed_and_expected("values")?;
    assert!(expected.len() > 5000, "the corpus shrank to {} values", expected.len());
    assert_eq!(printed, expected);
    Ok(())
}
