use super::*;

#[test]
fn rejects_empty() {
    let error = Principal::parse("").unwrap_err();
    assert_eq!(error.violation, PrincipalViolation::Empty);
}

#[test]
fn rejects_surrounding_whitespace() {
    for raw in [
        " operator@tenant_acme",
        "operator@tenant_acme ",
        "\toperator@tenant_acme",
        "operator@tenant_acme\n",
    ] {
        let error = Principal::parse(raw).unwrap_err();
        assert_eq!(error.violation, PrincipalViolation::SurroundingWhitespace, "{raw:?}");
    }
}

#[test]
fn rejects_over_long_principals() {
    let raw = "a".repeat(257);
    let error = Principal::parse(&raw).unwrap_err();
    assert_eq!(error.violation, PrincipalViolation::TooLong { max: 256, actual: 257 });
}

#[test]
fn accepts_a_human_principal() {
    let principal = Principal::parse("operator@tenant_acme").unwrap();
    assert_eq!(principal.as_str(), "operator@tenant_acme");
}

#[test]
fn accepts_a_machine_principal() {
    let principal = Principal::parse("verifier_run_5521@platform").unwrap();
    assert_eq!(principal.as_str(), "verifier_run_5521@platform");
}

#[test]
fn display_round_trips_through_parse() {
    for raw in ["operator@tenant_acme", "service@platform"] {
        let principal = Principal::parse(raw).unwrap();
        let rendered = format!("{principal}");
        assert_eq!(rendered.parse::<Principal>().unwrap(), principal);
    }
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let principal = Principal::parse("operator@tenant_acme").unwrap();

    assert_eq!("operator@tenant_acme".parse::<Principal>().unwrap(), principal);
    assert_eq!(principal.as_ref(), "operator@tenant_acme");
    assert_eq!(principal.to_string(), "operator@tenant_acme");
    assert_eq!(PrincipalViolation::Empty.to_string(), "must not be empty");
    assert_eq!(
        Principal::parse("").unwrap_err().to_string(),
        "principal '' is invalid: must not be empty"
    );
}
