use trogon_sandbox_contract::{
    DomainPattern, Host, InjectHosts, ResponseHeaderStrip, StrippedHeader, StrippedHeaderError,
};
use trogon_sandbox_protocol::{StripRule, fold_header_name, response_header_is_stripped};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn header(raw: &str) -> Result<StrippedHeader, StrippedHeaderError> {
    StrippedHeader::new(raw)
}

#[test]
fn names_fold_to_lower_case_with_dashes() -> TestResult {
    assert_eq!(header("Set_Cookie")?.as_str(), "set-cookie");
    assert_eq!(header("X.Request.Id")?.as_str(), "x-request-id");
    assert_eq!(fold_header_name(b"X.Request_Id"), "x-request-id");
    Ok(())
}

#[test]
fn framing_and_connection_headers_can_be_stripped() -> TestResult {
    for (name, folded) in [
        ("Content-Length", "content-length"),
        ("transfer_encoding", "transfer-encoding"),
        ("TE", "te"),
        ("Trailer", "trailer"),
        ("upgrade", "upgrade"),
        ("Connection", "connection"),
        ("keep_alive", "keep-alive"),
        ("Proxy-Connection", "proxy-connection"),
    ] {
        assert_eq!(header(name)?.as_str(), folded);
    }
    Ok(())
}

#[test]
fn matching_folds_the_response_header_name() -> TestResult {
    let strip = ResponseHeaderStrip::new([header("set-cookie")?]);
    assert!(response_header_is_stripped(&strip, b"Set-Cookie"));
    assert!(response_header_is_stripped(&strip, b"SET_COOKIE"));
    assert!(!response_header_is_stripped(&strip, b"x-kept"));
    assert!(!response_header_is_stripped(
        &ResponseHeaderStrip::default(),
        b"set-cookie"
    ));
    Ok(())
}

#[test]
fn a_rule_applies_at_the_hosts_it_names() -> TestResult {
    let rule = StripRule {
        hosts: InjectHosts::new(vec![
            "api.example.com:443".parse::<DomainPattern>()?,
            "*.corp.test".parse()?,
        ])?,
        headers: ResponseHeaderStrip::new([header("set-cookie")?]),
    };
    assert!(rule.applies_at(&"api.example.com".parse::<Host>()?, 443));
    assert!(!rule.applies_at(&"api.example.com".parse::<Host>()?, 80));
    assert!(rule.applies_at(&"a.corp.test".parse::<Host>()?, 8080));
    assert!(!rule.applies_at(&"other.test".parse::<Host>()?, 443));
    Ok(())
}
