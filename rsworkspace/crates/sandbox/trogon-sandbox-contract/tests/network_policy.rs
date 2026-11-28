use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    AllowRange, CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform,
    DeniedDomain, DomainPattern, DomainRule, DomainRuleAction, ForwardUrl, ForwardUrlError, HeaderMatcher, HeaderName,
    HttpMethod, IpRange, MatchRule, NetworkPolicy, OpaqueTunnelPolicy, PullSecretId, QueryMatcher, RequestForward,
    RequestRule, RequestRuleAction, ServiceAccountId, StringMatcher,
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
fn a_minimal_user_defined_policy_omits_its_optional_fields_but_states_its_tunnel_stance() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: OpaqueTunnelPolicy::Refuse,
        request_rules: Vec::new(),
    };
    let wire =
        json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "opaque_tunnels": "refuse"}});
    assert_eq!(serde_json::to_value(&policy)?, wire);
    assert_eq!(serde_json::from_value::<NetworkPolicy>(wire)?, policy);
    Ok(())
}

#[test]
fn a_user_defined_policy_that_does_not_mention_opaque_tunnels_refuses_them() -> TestResult {
    let wire = json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": []}});
    match serde_json::from_value::<NetworkPolicy>(wire)? {
        NetworkPolicy::UserDefined { opaque_tunnels, .. } => assert_eq!(opaque_tunnels, OpaqueTunnelPolicy::Refuse),
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

#[test]
fn an_opaque_tunnel_opt_in_is_written_out_so_no_reader_has_to_guess() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: OpaqueTunnelPolicy::Allow,
        request_rules: Vec::new(),
    };
    let wire = json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "opaque_tunnels": "allow"}});
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
                    forward_url: ForwardUrl::new("https://proxy.example.com")?,
                    match_rules: Vec::new(),
                    ttl_seconds: 30,
                }),
            },
        ],
        allow_ranges: vec![AllowRange::new("10.0.0.0/8")?, AllowRange::new("[fc00::1]:8080")?],
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
            "allow_ranges": ["10.0.0.0/8", "[fc00::1]:8080"],
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
fn opaque_tunnels_are_refused_by_default() -> TestResult {
    assert_eq!(OpaqueTunnelPolicy::default(), OpaqueTunnelPolicy::Refuse);
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
            methods: vec![HttpMethod::new("GET")?],
            query_string: vec![QueryMatcher {
                key: "token".to_string(),
                value: StringMatcher::Regex("^a+$".to_string()),
            }],
            headers: vec![HeaderMatcher {
                name: HeaderName::new("x-trace")?,
                value: StringMatcher::Exact("on".to_string()),
            }],
        }],
        credential: CredentialRequest {
            requester: Some(ServiceAccountId::new("svc-1")),
            ..credential_request()
        },
        injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
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
        "injection": {"header": "authorization"},
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
fn a_forward_url_has_no_query_and_no_fragment() -> TestResult {
    let url = ForwardUrl::new("https://proxy.example.com/relay")?;
    assert_eq!(url.as_str(), "https://proxy.example.com/relay");
    assert_eq!(url.to_string(), "https://proxy.example.com/relay");
    assert_eq!("https://proxy.example.com/relay".parse::<ForwardUrl>()?, url);
    assert_eq!(
        ForwardUrl::try_from("https://proxy.example.com/relay".to_string())?,
        url
    );
    assert_eq!(String::from(url.clone()), "https://proxy.example.com/relay");
    assert_eq!(serde_json::to_value(&url)?, json!("https://proxy.example.com/relay"));
    assert_eq!(
        serde_json::from_value::<ForwardUrl>(json!("https://proxy.example.com/relay"))?,
        url
    );

    let query = error_of(ForwardUrl::new("https://proxy.example.com/relay?debug=1"))?;
    assert_eq!(
        query,
        ForwardUrlError::HasQuery("https://proxy.example.com/relay?debug=1".to_string())
    );
    assert_eq!(
        query.to_string(),
        "forward_url 'https://proxy.example.com/relay?debug=1' must not include a query string"
    );
    let fragment = error_of(ForwardUrl::new("https://proxy.example.com/relay#section"))?;
    assert_eq!(
        fragment,
        ForwardUrlError::HasFragment("https://proxy.example.com/relay#section".to_string())
    );
    assert_eq!(
        fragment.to_string(),
        "forward_url 'https://proxy.example.com/relay#section' must not include a fragment"
    );
    assert_eq!(
        error_of(ForwardUrl::new("https://proxy.example.com/relay?"))?,
        ForwardUrlError::HasQuery("https://proxy.example.com/relay?".to_string())
    );
    let invalid = error_of(ForwardUrl::new("not a url"))?;
    assert_eq!(invalid.to_string(), "invalid forward_url 'not a url'");
    assert_eq!(
        std::error::Error::source(&invalid).map(ToString::to_string),
        Some("relative URL without a base".to_string())
    );
    for input in [
        "https://proxy.example.com/relay?debug=1",
        "https://proxy.example.com/relay#section",
        "not a url",
        "",
    ] {
        let wire = json!({"forward_url": input});
        assert_eq!(
            error_of(serde_json::from_value::<RequestForward>(wire.clone()))?.classify(),
            Data
        );
        assert_eq!(
            error_of(serde_json::from_str::<RequestForward>(&wire.to_string()))?.classify(),
            Data
        );
    }
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

#[test]
fn an_allow_range_reads_a_range_an_address_or_an_address_with_a_port() -> TestResult {
    for (value, cidr, port) in [
        ("10.0.0.0/8", "10.0.0.0/8", None),
        ("2001:0db8::/32", "2001:0db8::/32", None),
        ("10.0.0.6", "10.0.0.6/32", None),
        ("fc00::1", "fc00::1/128", None),
        ("10.0.0.5:8080", "10.0.0.5/32", Some(8080)),
        ("[fc00::1]:9", "fc00::1/128", Some(9)),
        ("[::ffff:10.0.0.5]:65535", "::ffff:10.0.0.5/128", Some(65535)),
    ] {
        let range = AllowRange::new(value)?;
        assert_eq!(range.as_str(), value);
        assert_eq!(range.to_string(), value);
        assert_eq!(range.cidr().as_str(), cidr);
        assert_eq!(range.port().map(|port| port.get()), port);
        assert_eq!(value.parse::<AllowRange>()?, range);
        assert_eq!(AllowRange::try_from(value.to_string())?, range);
        assert_eq!(serde_json::to_value(&range)?, json!(value));
        assert_eq!(serde_json::from_value::<AllowRange>(json!(value))?, range);
        assert_eq!(String::from(range), value);
    }
    Ok(())
}

#[test]
fn a_range_with_a_port_allows_that_port_only() -> TestResult {
    let scoped = AllowRange::new("10.0.0.5:8080")?;
    assert!(scoped.allows_port(8080));
    assert!(!scoped.allows_port(22));
    let unscoped = AllowRange::new("10.0.0.0/8")?;
    assert!(unscoped.allows_port(8080) && unscoped.allows_port(22));
    Ok(())
}

#[test]
fn an_ip_range_becomes_an_allow_range_for_every_port() -> TestResult {
    let range = AllowRange::from(IpRange::new("10.0.0.0/8")?);
    assert_eq!(range, AllowRange::new("10.0.0.0/8")?);
    assert_eq!(range.port(), None);
    Ok(())
}

#[test]
fn an_allow_range_rejects_what_is_not_a_range_an_address_or_a_socket_address() -> TestResult {
    for value in [
        "",
        "not a range",
        "10.0.0.0/33",
        "10.0.0.0/8:443",
        "10.0.0.5:",
        "10.0.0.5:0",
        "10.0.0.5:080",
        "10.0.0.5:65536",
        "10.0.0.5:+80",
        "10.0.0.5:http",
        "fc00::1]:80",
        "[fc00::1",
        "[fc00::1]",
        "[10.0.0.5]:80",
        "host:80",
        " 10.0.0.5",
    ] {
        let error = error_of(AllowRange::new(value))?;
        assert_eq!(error.input(), value);
        assert_eq!(
            error.to_string(),
            format!("'{value}' is not a CIDR range, an IP address, or an IP address with a port such as 10.0.0.5:8080")
        );
        assert_eq!(error_of(value.parse::<AllowRange>())?, error);
        assert_eq!(
            error_of(serde_json::from_value::<AllowRange>(json!(value)))?.classify(),
            Data
        );
    }
    let policy = json!({"user_defined": {"rules": [], "allow_ranges": ["10.0.0.5:0"], "deny_ranges": []}});
    assert_eq!(
        error_of(serde_json::from_value::<NetworkPolicy>(policy))?.classify(),
        Data
    );
    let policy = json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": ["10.0.0.5:80"]}});
    assert_eq!(
        error_of(serde_json::from_value::<NetworkPolicy>(policy))?.classify(),
        Data
    );
    Ok(())
}
