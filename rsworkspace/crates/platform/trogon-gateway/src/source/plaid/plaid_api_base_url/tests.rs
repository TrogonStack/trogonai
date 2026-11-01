use super::*;

#[test]
fn joins_the_key_endpoint_onto_a_bare_host() {
    let base = PlaidApiBaseUrl::new("https://production.plaid.com").unwrap();
    assert_eq!(
        base.webhook_verification_key_url().as_str(),
        "https://production.plaid.com/webhook_verification_key/get"
    );
}

#[test]
fn keeps_a_proxy_path_prefix() {
    let without_slash = PlaidApiBaseUrl::new("http://proxy.internal:8080/plaid").unwrap();
    let with_slash = PlaidApiBaseUrl::new("http://proxy.internal:8080/plaid/").unwrap();
    assert_eq!(
        without_slash.webhook_verification_key_url().as_str(),
        "http://proxy.internal:8080/plaid/webhook_verification_key/get"
    );
    assert_eq!(without_slash, with_slash);
}

#[test]
fn rejects_unparsable_urls() {
    let err = PlaidApiBaseUrl::new("not a url").unwrap_err();
    assert!(matches!(
        err,
        PlaidApiBaseUrlError::Parse(url::ParseError::RelativeUrlWithoutBase)
    ));
}

#[test]
fn rejects_non_http_schemes() {
    let err = PlaidApiBaseUrl::new("ftp://plaid.example").unwrap_err();
    assert!(matches!(err, PlaidApiBaseUrlError::UnsupportedScheme));
    assert_eq!(err.to_string(), "invalid Plaid API base URL: must use http or https");
}
