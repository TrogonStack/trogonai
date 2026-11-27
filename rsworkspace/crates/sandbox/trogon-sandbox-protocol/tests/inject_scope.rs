use trogon_sandbox_contract::{
    DomainPattern, Host, InjectMethod, InjectMethodSet, InjectPathPrefix, InjectScope, InjectScopeVersion,
};
use trogon_sandbox_protocol::{
    DEFAULT_INJECT_PORT, inject_effective_pattern, inject_host_matches, inject_omitted_port, inject_path_matches,
    inject_scope_admits, parse_inject_method,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn prefix(raw: &str) -> Result<InjectPathPrefix, Box<dyn std::error::Error>> {
    Ok(InjectPathPrefix::new(raw)?)
}

fn v2() -> InjectScope {
    InjectScope {
        scope_version: InjectScopeVersion::V2,
        ..InjectScope::default()
    }
}

#[test]
fn a_prefix_matches_on_segment_boundaries() -> TestResult {
    let v1 = prefix("/v1")?;
    assert!(inject_path_matches(&v1, "/v1"));
    assert!(inject_path_matches(&v1, "/v1/"));
    assert!(inject_path_matches(&v1, "/v1/users"));
    assert!(!inject_path_matches(&v1, "/v10"));
    assert!(!inject_path_matches(&v1, "/v10/users"));
    assert!(!inject_path_matches(&v1, "/v"));
    assert!(!inject_path_matches(&v1, "/other/v1"));
    Ok(())
}

#[test]
fn root_prefix_matches_every_path() -> TestResult {
    let root = prefix("/")?;
    assert!(inject_path_matches(&root, "/"));
    assert!(inject_path_matches(&root, "/anything"));
    assert!(!inject_path_matches(&root, "anything"));
    Ok(())
}

#[test]
fn ambiguous_request_paths_never_match() -> TestResult {
    let v1 = prefix("/v1")?;
    assert!(!inject_path_matches(&v1, "/v1/../admin"));
    assert!(!inject_path_matches(&v1, "/v1/%2e%2E/admin"));
    assert!(!inject_path_matches(&v1, "/v1/a%2Fb"));
    assert!(!inject_path_matches(&v1, "/v1/a%5Cb"));
    assert!(!inject_path_matches(&v1, "/v1/a%00b"));
    assert!(!inject_path_matches(&v1, "/v1\\x"));
    assert!(!inject_path_matches(&v1, "/v1/./x"));
    assert!(!inject_path_matches(&prefix("/")?, "/%2e/x"));
    Ok(())
}

#[test]
fn methods_are_case_sensitive_and_closed() {
    for (raw, method) in [
        ("GET", InjectMethod::Get),
        ("HEAD", InjectMethod::Head),
        ("POST", InjectMethod::Post),
        ("PUT", InjectMethod::Put),
        ("PATCH", InjectMethod::Patch),
        ("DELETE", InjectMethod::Delete),
        ("OPTIONS", InjectMethod::Options),
    ] {
        assert_eq!(parse_inject_method(raw), Some(method));
        assert_eq!(method.as_str(), raw);
    }
    assert_eq!(parse_inject_method("get"), None);
    assert_eq!(parse_inject_method("TRACE"), None);
}

#[test]
fn a_method_outside_the_set_or_the_enum_is_not_admitted() -> TestResult {
    let scope = InjectScope {
        methods: Some(InjectMethodSet::new(vec![InjectMethod::Get, InjectMethod::Head])?),
        ..InjectScope::default()
    };
    assert!(inject_scope_admits(&scope, "GET", "/"));
    assert!(inject_scope_admits(&scope, "HEAD", "/x"));
    assert!(!inject_scope_admits(&scope, "POST", "/"));
    assert!(!inject_scope_admits(&scope, "get", "/"));
    assert!(!inject_scope_admits(&scope, "PROPFIND", "/"));
    assert!(inject_scope_admits(&InjectScope::default(), "PROPFIND", "/"));
    Ok(())
}

#[test]
fn a_path_prefix_limits_the_scope() -> TestResult {
    let scope = InjectScope {
        path_prefix: Some(prefix("/v1")?),
        ..InjectScope::default()
    };
    assert!(inject_scope_admits(&scope, "GET", "/v1/users"));
    assert!(!inject_scope_admits(&scope, "GET", "/v2"));
    assert!(!inject_scope_admits(&scope, "GET", "/v1/../v2"));
    Ok(())
}

#[test]
fn an_omitted_port_is_any_port_in_v1_and_443_in_v2() -> TestResult {
    let pattern: DomainPattern = "api.example.com".parse()?;
    let host: Host = "api.example.com".parse()?;
    let v1 = InjectScope::default();
    let v2 = v2();
    assert!(inject_host_matches(&v1, &pattern, &host, 443));
    assert!(inject_host_matches(&v1, &pattern, &host, 8443));
    assert!(inject_host_matches(&v2, &pattern, &host, 443));
    assert!(!inject_host_matches(&v2, &pattern, &host, 8443));
    let explicit: DomainPattern = "api.example.com:8443".parse()?;
    assert!(inject_host_matches(&v2, &explicit, &host, 8443));
    assert!(!inject_host_matches(&v2, &explicit, &host, 443));
    assert_eq!(inject_omitted_port(InjectScopeVersion::V1), None);
    assert_eq!(inject_omitted_port(InjectScopeVersion::V2), Some(DEFAULT_INJECT_PORT));
    Ok(())
}

#[test]
fn the_effective_pattern_makes_an_omitted_port_explicit_in_v2() -> TestResult {
    let pattern: DomainPattern = "*.example.com".parse()?;
    assert_eq!(inject_effective_pattern(&InjectScope::default(), &pattern)?, pattern);
    assert_eq!(
        inject_effective_pattern(&v2(), &pattern)?.to_string(),
        "*.example.com:443"
    );
    let explicit: DomainPattern = "example.com:8443".parse()?;
    assert_eq!(inject_effective_pattern(&v2(), &explicit)?, explicit);
    Ok(())
}
