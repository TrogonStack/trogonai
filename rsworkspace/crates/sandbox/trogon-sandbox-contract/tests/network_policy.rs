use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform, DeniedDomain,
    DomainPattern, DomainRule, DomainRuleAction, HeaderMatcher, IpRange, MatchRule, NetworkPolicy, OpaqueTunnelPolicy,
    PullSecretId, QueryMatcher, RequestForward, RequestRule, RequestRuleAction, ServiceAccountId, StringMatcher,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn credential_request() -> CredentialRequest {
    CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new("registry-pull"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".to_string(),
        },
        ttl_seconds: 60,
        requester: None,
    }
}

#[test]
fn the_unrestricted_modes_are_bare_strings() -> TestResult {
    assert_eq!(serde_json::to_value(NetworkPolicy::AllowAll)?, json!("allow_all"));
    assert_eq!(serde_json::to_value(NetworkPolicy::DenyAll)?, json!("deny_all"));
    assert_eq!(
        serde_json::from_value::<NetworkPolicy>(json!("allow_all"))?,
        NetworkPolicy::AllowAll
    );
    assert_eq!(
        serde_json::from_value::<NetworkPolicy>(json!("deny_all"))?,
        NetworkPolicy::DenyAll
    );
    assert_eq!(
        error_of(serde_json::from_value::<NetworkPolicy>(json!("allow_some")))?.classify(),
        Data
    );
    Ok(())
}

#[test]
fn a_minimal_user_defined_policy_omits_its_optional_fields() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: OpaqueTunnelPolicy::Allow,
        request_rules: Vec::new(),
    };
    let wire = json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": []}});
    assert_eq!(serde_json::to_value(&policy)?, wire);
    assert_eq!(serde_json::from_value::<NetworkPolicy>(wire)?, policy);
    Ok(())
}

#[test]
fn a_full_user_defined_policy_round_trips() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![
            DomainRule {
                domain: DomainPattern::new("*.example.com")?,
                action: DomainRuleAction::PassThrough,
            },
            DomainRule {
                domain: DomainPattern::new("api.example.com:443")?,
                action: DomainRuleAction::Forward(RequestForward {
                    forward_url: "https://proxy.example.com".to_string(),
                    match_rules: Vec::new(),
                    ttl_seconds: 30,
                }),
            },
        ],
        allow_ranges: vec![IpRange::new("10.0.0.0/8")?],
        deny_ranges: vec![IpRange::new("10.1.0.0/16")?],
        denied_domains: vec![DeniedDomain::new("admin.example.com")?, DeniedDomain::new("*:22")?],
        opaque_tunnels: OpaqueTunnelPolicy::Refuse,
        request_rules: vec![RequestRule {
            action: RequestRuleAction::Deny,
            domains: Vec::new(),
            methods: Vec::new(),
            path_prefix: None,
            headers: Vec::new(),
        }],
    };
    let wire = json!({
        "user_defined": {
            "rules": [
                {"domain": "*.example.com", "action": "pass_through"},
                {
                    "domain": "api.example.com:443",
                    "action": {"forward": {
                        "forward_url": "https://proxy.example.com",
                        "match_rules": [],
                        "ttl_seconds": 30,
                    }},
                },
            ],
            "allow_ranges": ["10.0.0.0/8"],
            "deny_ranges": ["10.1.0.0/16"],
            "denied_domains": ["admin.example.com", "*:22"],
            "opaque_tunnels": "refuse",
            "request_rules": [{"action": "deny"}],
        }
    });
    assert_eq!(serde_json::to_value(&policy)?, wire);
    assert_eq!(serde_json::from_value::<NetworkPolicy>(wire.clone())?, policy);
    assert_eq!(serde_json::from_str::<NetworkPolicy>(&wire.to_string())?, policy);
    Ok(())
}

#[test]
fn a_user_defined_policy_rejects_a_bad_pattern() -> TestResult {
    let wire = json!({"user_defined": {
        "rules": [{"domain": "foo.*.com"}],
        "allow_ranges": [],
        "deny_ranges": [],
    }});
    assert_eq!(
        error_of(serde_json::from_value::<NetworkPolicy>(wire.clone()))?.classify(),
        Data
    );
    assert_eq!(
        error_of(serde_json::from_str::<NetworkPolicy>(&wire.to_string()))?.classify(),
        Data
    );

    let denied = json!({"user_defined": {
        "rules": [],
        "allow_ranges": [],
        "deny_ranges": [],
        "denied_domains": ["*.com"],
    }});
    assert_eq!(
        error_of(serde_json::from_value::<NetworkPolicy>(denied.clone()))?.classify(),
        Data
    );
    assert_eq!(
        error_of(serde_json::from_str::<NetworkPolicy>(&denied.to_string()))?.classify(),
        Data
    );

    let missing = json!({"user_defined": {"rules": []}});
    assert_eq!(
        error_of(serde_json::from_value::<NetworkPolicy>(missing))?.classify(),
        Data
    );
    Ok(())
}

#[test]
fn opaque_tunnels_allow_by_default() -> TestResult {
    assert_eq!(OpaqueTunnelPolicy::default(), OpaqueTunnelPolicy::Allow);
    assert!(OpaqueTunnelPolicy::Allow.is_allow());
    assert!(!OpaqueTunnelPolicy::Allow.refuses());
    assert!(!OpaqueTunnelPolicy::Refuse.is_allow());
    assert!(OpaqueTunnelPolicy::Refuse.refuses());
    assert_eq!(serde_json::to_value(OpaqueTunnelPolicy::Allow)?, json!("allow"));
    assert_eq!(serde_json::to_value(OpaqueTunnelPolicy::Refuse)?, json!("refuse"));
    assert_eq!(
        serde_json::from_value::<OpaqueTunnelPolicy>(json!("refuse"))?,
        OpaqueTunnelPolicy::Refuse
    );
    Ok(())
}

#[test]
fn a_domain_rule_defaults_to_pass_through() -> TestResult {
    let rule: DomainRule = serde_json::from_value(json!({"domain": "example.com"}))?;
    assert_eq!(rule.action, DomainRuleAction::PassThrough);
    assert_eq!(DomainRuleAction::default(), DomainRuleAction::PassThrough);
    assert_eq!(
        serde_json::to_value(&rule)?,
        json!({"domain": "example.com", "action": "pass_through"})
    );
    Ok(())
}

#[test]
fn a_transform_rule_carries_a_credential_and_an_injection() -> TestResult {
    let action = DomainRuleAction::Transform(CredentialTransform {
        match_rules: vec![MatchRule {
            path: Some(StringMatcher::StartsWith("/v1".to_string())),
            methods: vec!["GET".to_string()],
            query_string: vec![QueryMatcher {
                key: "token".to_string(),
                value: StringMatcher::Regex("^a+$".to_string()),
            }],
            headers: vec![HeaderMatcher {
                name: "x-trace".to_string(),
                value: StringMatcher::Exact("on".to_string()),
            }],
        }],
        credential: CredentialRequest {
            requester: Some(ServiceAccountId::new("svc-1")),
            ..credential_request()
        },
        injection: CredentialInjection::Header("Authorization".to_string()),
    });
    let wire = json!({"transform": {
        "match_rules": [{
            "path": {"starts_with": "/v1"},
            "methods": ["GET"],
            "query_string": [{"key": "token", "value": {"regex": "^a+$"}}],
            "headers": [{"name": "x-trace", "value": {"exact": "on"}}],
        }],
        "credential": {
            "provider": "registry",
            "pull_secret_id": "registry-pull",
            "scope": {"kind": "registry_pull", "repository_glob": "*"},
            "ttl_seconds": 60,
            "requester": "svc-1",
        },
        "injection": {"header": "Authorization"},
    }});
    assert_eq!(serde_json::to_value(&action)?, wire);
    assert_eq!(serde_json::from_value::<DomainRuleAction>(wire)?, action);
    Ok(())
}

#[test]
fn a_transform_without_match_rules_or_requester_uses_defaults() -> TestResult {
    let wire = json!({"transform": {
        "credential": {
            "provider": "s3",
            "pull_secret_id": "bucket-secret",
            "scope": {"kind": "s3_read_only", "bucket": "b", "prefix": "p"},
            "ttl_seconds": 5,
        },
        "injection": {"query": "key"},
    }});
    let action: DomainRuleAction = serde_json::from_value(wire)?;
    let DomainRuleAction::Transform(transform) = &action else {
        return Err("expected a transform".into());
    };
    assert!(transform.match_rules.is_empty());
    assert_eq!(transform.credential.requester, None);
    assert_eq!(transform.injection, CredentialInjection::Query("key".to_string()));
    assert_eq!(
        serde_json::to_value(&transform.credential)?,
        json!({
            "provider": "s3",
            "pull_secret_id": "bucket-secret",
            "scope": {"kind": "s3_read_only", "bucket": "b", "prefix": "p"},
            "ttl_seconds": 5,
        })
    );
    Ok(())
}

#[test]
fn a_forward_rule_defaults_its_ttl() -> TestResult {
    assert_eq!(RequestForward::DEFAULT_TTL_SECONDS, 60);
    assert_eq!(RequestForward::default_ttl_seconds(), 60);
    let forward: RequestForward = serde_json::from_value(json!({"forward_url": "https://proxy.example.com"}))?;
    assert_eq!(forward.ttl_seconds, 60);
    assert!(forward.match_rules.is_empty());
    assert_eq!(
        serde_json::to_value(&forward)?,
        json!({"forward_url": "https://proxy.example.com", "match_rules": [], "ttl_seconds": 60})
    );
    Ok(())
}

#[test]
fn a_match_rule_omits_what_it_does_not_match_on() -> TestResult {
    let empty = MatchRule {
        path: None,
        methods: Vec::new(),
        query_string: Vec::new(),
        headers: Vec::new(),
    };
    assert_eq!(serde_json::to_value(&empty)?, json!({}));
    assert_eq!(serde_json::from_value::<MatchRule>(json!({}))?, empty);
    Ok(())
}

#[test]
fn an_ip_range_parses_cidr_notation() -> TestResult {
    for (value, address, prefix_len) in [
        ("10.0.0.0/8", "10.0.0.0", 8),
        ("10.0.0.1/0", "10.0.0.1", 0),
        ("192.168.1.1/32", "192.168.1.1", 32),
        ("2001:db8::/32", "2001:db8::", 32),
        ("2001:0db8::/128", "2001:db8::", 128),
    ] {
        let range = IpRange::new(value)?;
        assert_eq!(range.as_str(), value);
        assert_eq!(range.to_string(), value);
        assert_eq!(range.address(), address.parse::<std::net::IpAddr>()?);
        assert_eq!(range.prefix_len(), prefix_len);
        assert_eq!(value.parse::<IpRange>()?, range);
        assert_eq!(IpRange::try_from(value.to_string())?, range);
        assert_eq!(serde_json::to_value(&range)?, json!(value));
        assert_eq!(serde_json::from_value::<IpRange>(json!(value))?, range);
        assert_eq!(String::from(range), value);
    }
    Ok(())
}

#[test]
fn an_ip_range_rejects_what_is_not_cidr() -> TestResult {
    for value in [
        "",
        "not a range",
        "10.0.0.0",
        "10.0.0.0/",
        "10.0.0.0/33",
        "2001:db8::/129",
        "10.0.0.0/08",
        "10.0.0.0/+8",
        "10.0.0.0/1000",
        "10.0.0.0/8/8",
        "host/8",
    ] {
        let error = error_of(IpRange::new(value))?;
        assert_eq!(error.input(), value);
        assert_eq!(
            error.to_string(),
            format!("'{value}' is not an IP range in CIDR notation such as 10.0.0.0/8")
        );
        assert_eq!(error_of(value.parse::<IpRange>())?, error);
        assert_eq!(
            error_of(serde_json::from_value::<IpRange>(json!(value)))?.classify(),
            Data
        );
    }
    let policy = json!({"user_defined": {"rules": [], "allow_ranges": ["10.0.0.0/999"], "deny_ranges": []}});
    assert_eq!(
        error_of(serde_json::from_value::<NetworkPolicy>(policy))?.classify(),
        Data
    );
    Ok(())
}
