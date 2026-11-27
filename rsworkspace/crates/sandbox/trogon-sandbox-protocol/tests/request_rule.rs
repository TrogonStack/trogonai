use trogon_sandbox_contract::{
    HeaderCondition, HeaderName, HeaderValue, Host, HttpMethod, PathPrefix, RequestRule, RequestRuleAction,
};
use trogon_sandbox_protocol::{
    RequestSummary, evaluate_request_rules, header_condition_is_met_by, path_prefix_covers, request_rule_matches,
    request_rule_selects,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn rule(action: RequestRuleAction) -> RequestRule {
    RequestRule {
        action,
        domains: Vec::new(),
        methods: Vec::new(),
        path_prefix: None,
        headers: Vec::new(),
    }
}

fn verdict(
    rules: &[RequestRule],
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<RequestRuleAction, Box<dyn std::error::Error>> {
    let host: Host = host.parse()?;
    let method = HttpMethod::new(method)?;
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    Ok(evaluate_request_rules(
        rules,
        &RequestSummary {
            host: &host,
            port: 443,
            method: &method,
            path,
            headers: &headers,
        },
    ))
}

#[test]
fn a_prefix_covers_itself_and_what_lies_below_it() -> TestResult {
    let v1 = PathPrefix::new("/v1")?;
    assert!(path_prefix_covers(&v1, "/v1"));
    assert!(path_prefix_covers(&v1, "/v1/"));
    assert!(path_prefix_covers(&v1, "/v1/users"));
    assert!(!path_prefix_covers(&v1, "/v1beta"));
    assert!(!path_prefix_covers(&v1, "/v"));
    assert!(!path_prefix_covers(&v1, "/other/v1"));
    assert!(path_prefix_covers(&PathPrefix::new("/")?, "/anything"));
    Ok(())
}

#[test]
fn the_query_is_not_part_of_the_path() -> TestResult {
    let admin = PathPrefix::new("/admin")?;
    assert!(path_prefix_covers(&admin, "/admin?x=/y"));
    assert!(path_prefix_covers(&admin, "/admin#frag"));
    assert!(!path_prefix_covers(&admin, "/administrator?x=1"));
    Ok(())
}

#[test]
fn spelling_cannot_step_around_a_prefix() -> TestResult {
    let admin = PathPrefix::new("/admin")?;
    assert!(path_prefix_covers(&admin, "/%61dmin"));
    assert!(!path_prefix_covers(&admin, "/%41dmin/x"));
    assert!(path_prefix_covers(&admin, "/admin%2Fsecret"));
    assert!(path_prefix_covers(&admin, "/admin%5csecret"));
    assert!(path_prefix_covers(&admin, "/admin\\secret"));
    assert!(path_prefix_covers(&admin, "/admin%2fsecret"));
    assert!(!path_prefix_covers(&admin, "/admin%zz"));
    assert!(!path_prefix_covers(&admin, "/admin%"));
    assert!(!path_prefix_covers(&admin, "/admin%20x"));
    assert!(path_prefix_covers(&admin, "/admin/%20x"));
    Ok(())
}

#[test]
fn repeated_slashes_cannot_step_around_a_prefix() -> TestResult {
    let api = PathPrefix::new("/api/admin")?;
    assert!(path_prefix_covers(&api, "/api//admin"));
    assert!(path_prefix_covers(&api, "/api///admin/x"));
    assert!(path_prefix_covers(&api, "/api/%2F/admin"));
    assert!(path_prefix_covers(&api, "/api%2f%5cadmin"));
    assert!(path_prefix_covers(&api, "/api\\\\admin"));
    assert!(path_prefix_covers(&api, "/api\\/admin"));
    assert!(!path_prefix_covers(&api, "/api//adminx"));
    Ok(())
}

#[test]
fn a_header_condition_matches_by_name_or_by_name_and_value() -> TestResult {
    let present = HeaderCondition {
        name: HeaderName::new("X-Trace")?,
        value: None,
    };
    let valued = HeaderCondition {
        name: HeaderName::new("x-trace")?,
        value: Some(HeaderValue::new("on")?),
    };
    let headers = vec![("X-TRACE".to_string(), " on ".to_string())];
    assert!(header_condition_is_met_by(&present, &headers));
    assert!(header_condition_is_met_by(&valued, &headers));
    assert!(!header_condition_is_met_by(
        &valued,
        &[("x-trace".to_string(), "off".to_string())]
    ));
    assert!(!header_condition_is_met_by(&present, &[]));
    assert!(header_condition_is_met_by(
        &valued,
        &[
            ("x-trace".to_string(), "off".to_string()),
            ("x-trace".to_string(), "on".to_string()),
        ]
    ));
    Ok(())
}

#[test]
fn the_first_matching_rule_decides_and_no_match_allows() -> TestResult {
    let rules = vec![
        RequestRule {
            methods: vec![HttpMethod::new("get")?],
            domains: vec!["api.example.com".parse()?],
            ..rule(RequestRuleAction::Allow)
        },
        RequestRule {
            domains: vec!["api.example.com".parse()?],
            ..rule(RequestRuleAction::Deny)
        },
    ];
    assert_eq!(
        verdict(&rules, "api.example.com", "GET", "/x", &[])?,
        RequestRuleAction::Allow
    );
    assert_eq!(
        verdict(&rules, "api.example.com", "POST", "/x", &[])?,
        RequestRuleAction::Deny
    );
    assert_eq!(
        verdict(&rules, "other.example.com", "POST", "/x", &[])?,
        RequestRuleAction::Allow
    );
    assert_eq!(verdict(&[], "a.test", "DELETE", "/", &[])?, RequestRuleAction::Allow);
    Ok(())
}

#[test]
fn a_deny_by_path_and_header_needs_both() -> TestResult {
    let rules = vec![RequestRule {
        path_prefix: Some(PathPrefix::new("/admin")?),
        headers: vec![HeaderCondition {
            name: HeaderName::new("x-role")?,
            value: Some(HeaderValue::new("root")?),
        }],
        ..rule(RequestRuleAction::Deny)
    }];
    let root = [("X-Role", "root")];
    assert_eq!(
        verdict(&rules, "a.test", "GET", "/admin/x", &root)?,
        RequestRuleAction::Deny
    );
    assert_eq!(
        verdict(&rules, "a.test", "GET", "/public", &root)?,
        RequestRuleAction::Allow
    );
    assert_eq!(
        verdict(&rules, "a.test", "GET", "/admin/x", &[])?,
        RequestRuleAction::Allow
    );
    Ok(())
}

#[test]
fn a_rule_selects_the_destinations_it_names() -> TestResult {
    let unrestricted = rule(RequestRuleAction::Deny);
    let scoped = RequestRule {
        domains: vec!["*.example.com:443".parse()?],
        ..rule(RequestRuleAction::Deny)
    };
    let host: Host = "api.example.com".parse()?;
    assert!(request_rule_selects(&unrestricted, &host, 1));
    assert!(request_rule_selects(&scoped, &host, 443));
    assert!(!request_rule_selects(&scoped, &host, 80));
    let method = HttpMethod::new("GET")?;
    let summary = RequestSummary {
        host: &host,
        port: 80,
        method: &method,
        path: "/",
        headers: &[],
    };
    assert!(!request_rule_matches(&scoped, &summary));
    assert!(request_rule_matches(&unrestricted, &summary));
    Ok(())
}
