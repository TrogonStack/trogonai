use std::error::Error;

use super::*;
use trogon_sandbox_contract::{
    AllowRange, CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform,
    ForwardUrl, HeaderMatcher, HeaderName, HttpMethod, IpRange, OpaqueTunnelPolicy, PullSecretId, QueryMatcher,
    RequestForward,
};

type TestResult = Result<(), Box<dyn Error>>;

fn credential_transform(match_rules: Vec<MatchRule>) -> Result<CredentialTransform, Box<dyn Error>> {
    Ok(CredentialTransform {
        match_rules,
        credential: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new("test-secret"),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
        injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
    })
}

fn policy(rules: Vec<DomainRule>, allow: Vec<IpRange>, deny: Vec<IpRange>) -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules,
        allow_ranges: allow.into_iter().map(AllowRange::from).collect(),
        deny_ranges: deny,
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::default(),
        request_rules: vec![],
    }
}

fn path_rule(matcher: StringMatcher) -> MatchRule {
    MatchRule {
        path: Some(matcher),
        methods: vec![],
        query_string: vec![],
        headers: vec![],
    }
}

fn transform_rule(match_rules: Vec<MatchRule>) -> Result<DomainRule, Box<dyn Error>> {
    Ok(DomainRule {
        domain: "example.com".parse()?,
        action: DomainRuleAction::Transform(credential_transform(match_rules)?),
    })
}

#[test]
fn allow_all_and_deny_all_are_valid() {
    assert!(validate_network_policy(&NetworkPolicy::AllowAll).is_ok());
    assert!(validate_network_policy(&NetworkPolicy::DenyAll).is_ok());
}

#[test]
fn user_defined_with_good_fields_is_valid() -> TestResult {
    let full_rule = MatchRule {
        path: Some(StringMatcher::Regex("^/v1/.*$".to_string())),
        methods: vec![HttpMethod::new("GET")?],
        query_string: vec![QueryMatcher {
            key: "page".to_string(),
            value: StringMatcher::Regex("^[0-9]+$".to_string()),
        }],
        headers: vec![HeaderMatcher {
            name: HeaderName::new("x-request-id")?,
            value: StringMatcher::Regex("^[0-9a-f]{8}$".to_string()),
        }],
    };
    let rules = vec![
        DomainRule {
            domain: "example.com".parse()?,
            action: DomainRuleAction::PassThrough,
        },
        DomainRule {
            domain: "*.api.example.com".parse()?,
            action: DomainRuleAction::Transform(credential_transform(vec![full_rule])?),
        },
        DomainRule {
            domain: "internal.example.com".parse()?,
            action: DomainRuleAction::Forward(RequestForward {
                forward_url: ForwardUrl::new("https://proxy.example.com/relay")?,
                match_rules: vec![],
                ttl_seconds: RequestForward::DEFAULT_TTL_SECONDS,
            }),
        },
    ];
    let allow = vec![IpRange::new("10.0.0.0/8")?, IpRange::new("2001:db8::/32")?];
    let deny = vec![IpRange::new("10.1.0.0/24")?];
    assert!(validate_network_policy(&policy(rules, allow, deny)).is_ok());
    Ok(())
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the test asserts the operator-facing message text itself"
    )
)]
fn bad_regex_in_path_matcher_is_rejected() -> TestResult {
    let rule = transform_rule(vec![path_rule(StringMatcher::Regex("(unclosed".to_string()))])?;
    let error = validate_network_policy(&policy(vec![rule], vec![], vec![]))
        .err()
        .ok_or("expected a rejection")?;
    assert!(matches!(
        error,
        NetworkPolicyValidationError::InvalidRegex { ref pattern, .. } if pattern == "(unclosed"
    ));
    assert!(error.to_string().starts_with("invalid regex '(unclosed'"));
    Ok(())
}

#[test]
fn exact_and_prefix_matchers_need_no_regex() -> TestResult {
    let rules = vec![transform_rule(vec![
        path_rule(StringMatcher::Exact("(".to_string())),
        path_rule(StringMatcher::StartsWith("[".to_string())),
    ])?];
    assert!(validate_network_policy(&policy(rules, vec![], vec![])).is_ok());
    Ok(())
}

#[test]
fn bad_regex_in_query_matcher_is_rejected() -> TestResult {
    let rule = transform_rule(vec![MatchRule {
        path: None,
        methods: vec![],
        query_string: vec![QueryMatcher {
            key: "q".to_string(),
            value: StringMatcher::Regex("(".to_string()),
        }],
        headers: vec![],
    }])?;
    assert!(validate_network_policy(&policy(vec![rule], vec![], vec![])).is_err());
    Ok(())
}

#[test]
fn bad_regex_in_forward_header_matcher_is_rejected() -> TestResult {
    let rule = DomainRule {
        domain: "example.com".parse()?,
        action: DomainRuleAction::Forward(RequestForward {
            forward_url: ForwardUrl::new("https://proxy.example.com")?,
            match_rules: vec![MatchRule {
                path: None,
                methods: vec![],
                query_string: vec![],
                headers: vec![HeaderMatcher {
                    name: HeaderName::new("x-id")?,
                    value: StringMatcher::Regex("[".to_string()),
                }],
            }],
            ttl_seconds: RequestForward::DEFAULT_TTL_SECONDS,
        }),
    };
    assert!(validate_network_policy(&policy(vec![rule], vec![], vec![])).is_err());
    Ok(())
}

#[test]
fn malformed_ranges_and_forward_urls_are_unrepresentable() {
    assert!(IpRange::new("not-a-cidr").is_err());
    assert!(IpRange::new("10.0.0.0/999").is_err());
    assert!(ForwardUrl::new("https://x.example.com/?q=1").is_err());
    assert!(ForwardUrl::new("https://x.example.com/#frag").is_err());
}
