use super::*;
use crate::source::plaid::PlaidApiBaseUrl;

#[test]
fn parses_known_environments() {
    assert_eq!(PlaidEnvironment::parse("sandbox").unwrap(), PlaidEnvironment::Sandbox);
    assert_eq!(
        PlaidEnvironment::parse("production").unwrap(),
        PlaidEnvironment::Production
    );
}

#[test]
fn rejects_unknown_environments() {
    for value in ["", "development", "Production", "SANDBOX", " sandbox"] {
        assert!(PlaidEnvironment::parse(value).is_err(), "{value:?}");
    }
}

#[test]
fn error_display_names_the_value_and_the_choices() {
    let err = PlaidEnvironment::parse("development").unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown environment \"development\", expected sandbox or production"
    );
}

#[test]
fn display_roundtrips_through_parse() {
    for environment in [PlaidEnvironment::Sandbox, PlaidEnvironment::Production] {
        assert_eq!(PlaidEnvironment::parse(&environment.to_string()).unwrap(), environment);
    }
}

#[test]
fn default_api_base_urls_are_valid_and_distinct() {
    assert_eq!(
        PlaidEnvironment::Sandbox.default_api_base_url(),
        "https://sandbox.plaid.com"
    );
    assert_eq!(
        PlaidEnvironment::Production.default_api_base_url(),
        "https://production.plaid.com"
    );
    for environment in [PlaidEnvironment::Sandbox, PlaidEnvironment::Production] {
        assert!(PlaidApiBaseUrl::new(environment.default_api_base_url()).is_ok());
    }
}
