use super::*;
use std::net::Ipv4Addr;
use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform, DomainRuleAction,
    ForwardUrl, HeaderName, MatchRule, PullSecretId,
};

fn domain_rule(domain: &str) -> Result<DomainRule, Box<dyn std::error::Error>> {
    Ok(DomainRule {
        domain: domain.parse()?,
        action: DomainRuleAction::PassThrough,
    })
}

fn ip4(s: &str) -> Result<IpAddr, Box<dyn std::error::Error>> {
    Ok(IpAddr::V4(s.parse::<Ipv4Addr>()?))
}

pub(crate) fn test_credential_transform(
    match_rules: Vec<MatchRule>,
) -> Result<CredentialTransform, Box<dyn std::error::Error>> {
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

fn matches(pattern: &str, host: &str, port: u16) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(domain_matches(&DomainPattern::new(pattern)?, &Host::new(host)?, port))
}

fn user_defined(
    allow: &[&str],
    denied: &[&str],
    allow_ranges: &[&str],
    deny_ranges: &[&str],
) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: allow.iter().map(|d| domain_rule(d)).collect::<Result<_, _>>()?,
        allow_ranges: allow_ranges
            .iter()
            .map(|r| AllowRange::new(*r))
            .collect::<Result<_, _>>()?,
        deny_ranges: deny_ranges.iter().map(|r| IpRange::new(*r)).collect::<Result<_, _>>()?,
        denied_domains: denied.iter().map(|d| d.parse()).collect::<Result<_, _>>()?,
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

#[test]
fn wildcard_matches_subdomain() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches("*.example.com", "api.example.com", 443)?);
    assert!(matches("*.example.com", "a.b.example.com", 443)?);
    Ok(())
}

#[test]
fn wildcard_does_not_match_bare_domain() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!matches("*.example.com", "example.com", 443)?);
    Ok(())
}

#[test]
fn wildcard_does_not_match_lookalike_suffix() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!matches("*.example.com", "evil-example.com", 443)?);
    assert!(!matches("*.example.com", "notexample.com", 443)?);
    Ok(())
}

#[test]
fn exact_pattern_matches_only_exact_host() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches("example.com", "example.com", 443)?);
    assert!(!matches("example.com", "api.example.com", 443)?);
    Ok(())
}

#[test]
fn denied_domain_wins_over_allowed_domain() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["*.example.com"], &["admin.example.com"], &[], &[])?;
    assert_eq!(
        decide(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Allow
    );
    assert_eq!(
        decide(&policy, Some("admin.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn denied_domain_wins_over_allowed_ip_range() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&[], &["blocked.example.com"], &["203.0.113.0/24"], &[])?;
    assert_eq!(
        decide(&policy, Some("blocked.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Deny
    );
    assert_eq!(
        decide(&policy, Some("fine.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Allow
    );
    Ok(())
}

#[test]
fn denied_domain_order_does_not_matter() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["a.example.com", "*.example.com"], &["a.example.com"], &[], &[])?;
    assert_eq!(
        decide(&policy, Some("a.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn allowed_port_restricts_the_rule() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["api.example.com:443"], &[], &[], &[])?;
    assert_eq!(
        decide(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Allow
    );
    assert_eq!(
        decide(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 22),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn denied_port_only_blocks_that_port() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["*.example.com"], &["*.example.com:22"], &[], &[])?;
    assert_eq!(
        decide(&policy, Some("git.example.com"), ip4("203.0.113.5")?, 22),
        Decision::Deny
    );
    assert_eq!(
        decide(&policy, Some("git.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Allow
    );
    Ok(())
}

#[test]
fn bare_star_denies_every_destination_on_a_port() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["*.example.com"], &["*:22"], &["0.0.0.0/0"], &[])?;
    assert_eq!(
        decide(&policy, Some("git.example.com"), ip4("203.0.113.5")?, 22),
        Decision::Deny
    );
    assert_eq!(decide(&policy, None, ip4("203.0.113.5")?, 22), Decision::Deny);
    assert_eq!(decide(&policy, None, ip4("203.0.113.5")?, 443), Decision::Allow);
    Ok(())
}

#[test]
fn sni_is_canonicalised_before_matching() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["api.example.com"], &[], &[], &[])?;
    for sni in ["API.Example.COM", "api.example.com."] {
        assert_eq!(
            decide(&policy, Some(sni), ip4("203.0.113.5")?, 443),
            Decision::Allow,
            "{sni}"
        );
    }
    let denied = user_defined(&["*.example.com"], &["admin.example.com"], &[], &[])?;
    assert_eq!(
        decide(&denied, Some("ADMIN.example.com."), ip4("203.0.113.5")?, 443),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn idna_sni_matches_punycode_pattern() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["xn--bcher-kva.example"], &[], &[], &[])?;
    assert_eq!(
        decide(&policy, Some("b\u{fc}cher.example"), ip4("203.0.113.5")?, 443),
        Decision::Allow
    );
    Ok(())
}

#[test]
fn malformed_sni_is_denied_even_when_an_ip_range_allows() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&[], &[], &["203.0.113.0/24"], &[])?;
    assert_eq!(
        decide(&policy, Some("evil.com\u{0}.allowed.com"), ip4("203.0.113.5")?, 443),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn denied_ip_literal_blocks_by_destination_address() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["*.example.com"], &["169.254.169.254"], &[], &[])?;
    assert_eq!(
        decide(&policy, Some("api.example.com"), ip4("169.254.169.254")?, 80),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn a_port_qualified_denied_literal_blocks_that_port_of_a_name_only() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["git.example.com"], &["10.0.0.5:22"], &[], &[])?;
    assert_eq!(
        decide(&policy, Some("git.example.com"), ip4("10.0.0.5")?, 22),
        Decision::Deny
    );
    assert_eq!(
        decide(&policy, Some("git.example.com"), ip4("10.0.0.5")?, 443),
        Decision::Allow
    );
    Ok(())
}

#[test]
fn a_denied_literal_beats_an_allowed_one_and_the_localhost_rule_per_port() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(
        &["myapp.test", "127.0.0.1", "10.0.0.5", "localhost"],
        &["127.0.0.1:6379", "10.0.0.5:22", "[::1]:6379"],
        &[],
        &[],
    )?;
    let cases = [
        ("myapp.test", "127.0.0.1", 3000, Decision::Allow),
        ("myapp.test", "127.0.0.1", 6379, Decision::Deny),
        ("myapp.test", "10.0.0.5", 443, Decision::Allow),
        ("myapp.test", "10.0.0.5", 22, Decision::Deny),
        ("localhost", "127.0.0.1", 8080, Decision::Allow),
        ("localhost", "127.0.0.1", 6379, Decision::Deny),
    ];
    for (name, address, port, expected) in cases {
        assert_eq!(
            decide(&policy, Some(name), ip4(address)?, port),
            expected,
            "{name} {address}:{port}"
        );
    }
    assert_eq!(
        decide(&policy, Some("app.localhost"), "::1".parse()?, 6379),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn a_denied_literal_refuses_the_addresses_that_carry_it() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&[], &["127.0.0.1", "10.0.0.5:22", "[::ffff:10.0.0.2]"], &[], &[])?;
    let refused = [
        ("127.0.0.1", 443),
        ("::ffff:127.0.0.1", 443),
        ("::7f00:1", 443),
        ("::ffff:0:7f00:1", 443),
        ("64:ff9b::7f00:1", 443),
        ("64:ff9b::127.0.0.1", 443),
        ("2002:7f00:1:5::9", 443),
        ("64:ff9b::a00:5", 22),
        ("10.0.0.2", 443),
        ("::ffff:10.0.0.2", 443),
    ];
    for (address, port) in refused {
        assert!(refuses_address(&policy, address.parse()?, port), "{address}:{port}");
    }
    let permitted = [
        ("64:ff9b::a00:5", 443),
        ("64:ff9b:1::7f00:1", 443),
        ("64:ff9b:0:1::7f00:1", 443),
        ("2001:db8::7f00:1", 443),
        ("fe80::7f00:1", 443),
        ("10.0.0.3", 443),
        ("192.0.2.1", 443),
    ];
    for (address, port) in permitted {
        assert!(!refuses_address(&policy, address.parse()?, port), "{address}:{port}");
    }
    Ok(())
}

#[test]
fn a_bare_denied_literal_beats_a_port_scoped_allowed_one() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["myapp.test", "127.0.0.1:3000"], &["127.0.0.1"], &[], &[])?;
    for port in [3000, 5432] {
        assert_eq!(
            decide(&policy, Some("myapp.test"), ip4("127.0.0.1")?, port),
            Decision::Deny,
            "{port}"
        );
    }
    Ok(())
}

#[test]
fn denied_literals_and_wildcard_ports_refuse_a_name_that_resolves_to_them() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["*.example.com"], &["10.0.0.5", "[fd00::7]:22", "*:25"], &[], &[])?;
    let cases = [
        ("intranet.example.com", ip4("10.0.0.5")?, 443, Decision::Deny),
        ("intranet.example.com", "fd00::7".parse()?, 22, Decision::Deny),
        ("intranet.example.com", "fd00::7".parse()?, 443, Decision::Allow),
        ("intranet.example.com", ip4("203.0.113.9")?, 25, Decision::Deny),
    ];
    for (name, address, port, expected) in cases {
        assert_eq!(
            decide(&policy, Some(name), address, port),
            expected,
            "{name} {address}:{port}"
        );
    }
    Ok(())
}

#[test]
fn allowed_ip_literal_permits_that_address_only() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["203.0.113.5:443"], &[], &[], &[])?;
    assert_eq!(decide(&policy, None, ip4("203.0.113.5")?, 443), Decision::Allow);
    assert_eq!(decide(&policy, None, ip4("203.0.113.5")?, 80), Decision::Deny);
    assert_eq!(decide(&policy, None, ip4("203.0.113.6")?, 443), Decision::Deny);
    Ok(())
}

#[test]
fn wildcard_rule_never_allows_an_ip_destination() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["*.example.com"], &[], &[], &[])?;
    assert_eq!(decide(&policy, None, ip4("203.0.113.5")?, 443), Decision::Deny);
    Ok(())
}

#[test]
fn allow_all_always_allows() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::AllowAll;
    assert_eq!(decide(&policy, None, ip4("1.2.3.4")?, 443), Decision::Allow);
    assert_eq!(
        decide(&policy, Some("evil.example"), ip4("10.0.0.1")?, 80),
        Decision::Allow
    );
    Ok(())
}

#[test]
fn deny_all_always_denies() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::DenyAll;
    assert_eq!(decide(&policy, None, ip4("1.2.3.4")?, 443), Decision::Deny);
    assert_eq!(
        decide(&policy, Some("good.example.com"), ip4("10.0.0.1")?, 80),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn user_defined_allows_matching_sni_domain() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![domain_rule("*.example.com")?],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert_eq!(
        decide(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Allow
    );
    assert_eq!(
        decide(&policy, Some("other.test"), ip4("203.0.113.5")?, 443),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn user_defined_allows_matching_ip_range() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("10.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert_eq!(decide(&policy, None, ip4("10.1.2.3")?, 5432), Decision::Allow);
    assert_eq!(decide(&policy, None, ip4("192.168.1.1")?, 5432), Decision::Deny);
    Ok(())
}

#[test]
fn an_allow_range_with_a_port_allows_that_port_only() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("10.0.0.5:5432")?, AllowRange::new("[fc00::1]:9")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert_eq!(decide(&policy, None, ip4("10.0.0.5")?, 5432), Decision::Allow);
    assert_eq!(decide(&policy, None, ip4("10.0.0.5")?, 22), Decision::Deny);
    assert_eq!(decide(&policy, None, ip4("10.0.0.6")?, 5432), Decision::Deny);
    assert_eq!(decide(&policy, None, "fc00::1".parse::<IpAddr>()?, 9), Decision::Allow);
    assert_eq!(
        decide(&policy, None, "fc00::1".parse::<IpAddr>()?, 5432),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn deny_ranges_take_precedence_over_allow_ranges() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("10.0.0.0/8")?],
        deny_ranges: vec![IpRange::new("10.1.0.0/16")?],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    // In both ranges: deny wins.
    assert_eq!(decide(&policy, None, ip4("10.1.2.3")?, 443), Decision::Deny);
    // Only in allow range: allowed.
    assert_eq!(decide(&policy, None, ip4("10.2.2.3")?, 443), Decision::Allow);
    Ok(())
}

#[test]
fn deny_ranges_take_precedence_over_matching_sni() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![domain_rule("api.example.com")?],
        allow_ranges: vec![],
        deny_ranges: vec![IpRange::new("203.0.113.0/24")?],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert_eq!(
        decide(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 443),
        Decision::Deny
    );
    Ok(())
}

#[test]
fn user_defined_denies_by_default_with_no_sni_and_no_range_match() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![domain_rule("api.example.com")?],
        allow_ranges: vec![AllowRange::new("10.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert_eq!(decide(&policy, None, ip4("198.51.100.7")?, 443), Decision::Deny);
    Ok(())
}

#[test]
fn decide_with_action_returns_pass_through_action_on_domain_match() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![domain_rule("api.example.com")?],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let (decision, action) = decide_with_action(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 443);
    assert_eq!(decision, Decision::Allow);
    assert_eq!(action, Some(DomainRuleAction::PassThrough));
    Ok(())
}

#[test]
fn decide_with_action_returns_transform_action_on_domain_match() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: "api.example.com".parse()?,
            action: DomainRuleAction::Transform(test_credential_transform(vec![])?),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let (decision, action) = decide_with_action(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 443);
    assert_eq!(decision, Decision::Allow);
    assert!(matches!(action, Some(DomainRuleAction::Transform(_))));
    Ok(())
}

#[test]
fn decide_with_action_returns_forward_action_on_domain_match() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: "api.example.com".parse()?,
            action: DomainRuleAction::Forward(trogon_sandbox_contract::RequestForward {
                forward_url: ForwardUrl::new("https://forward.example.com")?,
                match_rules: vec![],
                ttl_seconds: trogon_sandbox_contract::RequestForward::DEFAULT_TTL_SECONDS,
            }),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let (decision, action) = decide_with_action(&policy, Some("api.example.com"), ip4("203.0.113.5")?, 443);
    assert_eq!(decision, Decision::Allow);
    assert!(matches!(action, Some(DomainRuleAction::Forward(_))));
    Ok(())
}

#[test]
fn decide_with_action_returns_none_on_ip_range_match() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("10.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let (decision, action) = decide_with_action(&policy, None, ip4("10.1.2.3")?, 443);
    assert_eq!(decision, Decision::Allow);
    assert_eq!(action, None);
    Ok(())
}

#[test]
fn decide_with_action_returns_none_on_allow_all() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::AllowAll;
    let (decision, action) = decide_with_action(&policy, Some("api.example.com"), ip4("10.1.2.3")?, 443);
    assert_eq!(decision, Decision::Allow);
    assert_eq!(action, None);
    Ok(())
}

#[test]
fn decide_with_action_returns_none_on_deny() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![domain_rule("api.example.com")?],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let (decision, action) = decide_with_action(&policy, Some("other.example.net"), ip4("203.0.113.5")?, 443);
    assert_eq!(decision, Decision::Deny);
    assert_eq!(action, None);
    Ok(())
}

#[test]
fn decide_host_screens_names_without_addresses() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["allowed.test"], &["blocked.test"], &[], &["10.0.0.0/8"])?;
    let host = |h: &str| Host::new(h);
    assert_eq!(decide_host(&policy, &host("allowed.test")?, 443), Decision::Allow);
    assert_eq!(decide_host(&policy, &host("blocked.test")?, 443), Decision::Deny);
    assert_eq!(decide_host(&policy, &host("other.test")?, 443), Decision::Deny);
    assert_eq!(decide_host(&policy, &host("10.1.1.1")?, 443), Decision::Deny);
    assert_eq!(decide_host(&policy, &host("8.8.8.8")?, 443), Decision::Deny);
    Ok(())
}

#[test]
fn decide_host_defers_unlisted_names_to_allow_ranges() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&[], &[], &["203.0.113.0/24"], &[])?;
    let other = Host::new("other.test")?;
    assert_eq!(decide_host(&policy, &other, 443), Decision::Allow);
    assert_eq!(decide_host(&policy, &Host::new("203.0.113.9")?, 443), Decision::Allow);
    assert_eq!(decide_host(&policy, &Host::new("198.51.100.1")?, 443), Decision::Deny);
    Ok(())
}

#[test]
fn decide_host_covers_the_trivial_policies() -> Result<(), Box<dyn std::error::Error>> {
    let host = Host::new("a.test")?;
    assert_eq!(decide_host(&NetworkPolicy::AllowAll, &host, 1), Decision::Allow);
    assert_eq!(decide_host(&NetworkPolicy::DenyAll, &host, 1), Decision::Deny);
    Ok(())
}

fn sni_policy(allow_ranges: &[&str]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![domain_rule("*.example.com")?],
        allow_ranges: allow_ranges
            .iter()
            .map(|r| AllowRange::new(*r))
            .collect::<Result<_, _>>()?,
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

#[test]
fn an_allow_through_a_rule_the_sni_matches_rests_on_the_sni() -> Result<(), Box<dyn std::error::Error>> {
    assert!(allow_rests_on_sni(
        &sni_policy(&[])?,
        Some("api.example.com"),
        ip4("203.0.113.7")?,
        443
    ));
    Ok(())
}

#[test]
fn an_allow_that_does_not_go_through_the_sni_does_not_rest_on_it() -> Result<(), Box<dyn std::error::Error>> {
    let by_range = sni_policy(&["203.0.113.0/24"])?;
    assert!(!allow_rests_on_sni(
        &by_range,
        Some("other.test"),
        ip4("203.0.113.7")?,
        443
    ));
    assert!(!allow_rests_on_sni(&by_range, None, ip4("203.0.113.7")?, 443));
    assert!(!allow_rests_on_sni(
        &NetworkPolicy::AllowAll,
        Some("api.example.com"),
        ip4("203.0.113.7")?,
        443
    ));
    Ok(())
}

#[test]
fn an_allow_through_an_address_rule_does_not_rest_on_the_sni() -> Result<(), Box<dyn std::error::Error>> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![domain_rule("203.0.113.7")?],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert!(!allow_rests_on_sni(
        &policy,
        Some("evil.test"),
        ip4("203.0.113.7")?,
        443
    ));
    Ok(())
}

#[test]
fn rule_separates_a_forbidden_destination_from_an_unlisted_one() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["allowed.test"], &["blocked.test"], &[], &["10.0.0.0/8"])?;
    let public = ip4("203.0.113.5")?;
    assert_eq!(rule(&policy, Some("unknown.test"), public, 443), Ruling::Unlisted);
    assert_eq!(rule(&policy, None, public, 443), Ruling::Unlisted);
    assert_eq!(rule(&policy, Some("blocked.test"), public, 443), Ruling::Deny);
    assert_eq!(rule(&policy, Some("unknown.test"), ip4("10.1.1.1")?, 443), Ruling::Deny);
    assert_eq!(
        rule(&policy, Some("allowed.test"), public, 443),
        Ruling::Allow(Some(DomainRuleAction::PassThrough))
    );
    Ok(())
}

#[test]
fn rule_treats_a_malformed_sni_as_a_final_deny() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["allowed.test"], &[], &[], &[])?;
    assert_eq!(
        rule(&policy, Some("not a host"), ip4("203.0.113.5")?, 443),
        Ruling::Deny
    );
    Ok(())
}

#[test]
fn rule_covers_the_trivial_policies() -> Result<(), Box<dyn std::error::Error>> {
    let public = ip4("203.0.113.5")?;
    assert_eq!(rule(&NetworkPolicy::AllowAll, None, public, 1), Ruling::Allow(None));
    assert_eq!(rule(&NetworkPolicy::DenyAll, None, public, 1), Ruling::Deny);
    Ok(())
}

#[test]
fn rule_host_leaves_an_unlisted_name_unlisted_and_denies_the_denied() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["allowed.test"], &["blocked.test"], &["203.0.113.0/24"], &[])?;
    let host = |name: &str| Host::new(name);
    assert_eq!(rule_host(&policy, &host("other.test")?, 443), Ruling::Unlisted);
    assert_eq!(rule_host(&policy, &host("blocked.test")?, 443), Ruling::Deny);
    assert_eq!(
        rule_host(&policy, &host("allowed.test")?, 443),
        Ruling::Allow(Some(DomainRuleAction::PassThrough))
    );
    assert_eq!(rule_host(&policy, &host("198.51.100.1")?, 443), Ruling::Unlisted);
    assert_eq!(rule_host(&policy, &host("203.0.113.9")?, 443), Ruling::Allow(None));
    Ok(())
}

#[test]
fn rule_host_denies_a_deny_range_ip_literal() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&[], &[], &[], &["10.0.0.0/8"])?;
    assert_eq!(rule_host(&policy, &Host::new("10.1.2.3")?, 443), Ruling::Deny);
    Ok(())
}

#[test]
fn allows_by_address_needs_a_user_defined_policy_with_allow_ranges() -> Result<(), Box<dyn std::error::Error>> {
    assert!(allows_by_address(&user_defined(&[], &[], &["10.0.0.0/8"], &[])?, 443));
    assert!(!allows_by_address(&user_defined(&[], &[], &[], &[])?, 443));
    assert!(!allows_by_address(&NetworkPolicy::AllowAll, 443));
    assert!(!allows_by_address(&NetworkPolicy::DenyAll, 443));
    Ok(())
}

#[test]
fn allows_by_address_needs_an_allow_range_open_on_the_port() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&[], &[], &["203.0.113.7:8443"], &[])?;
    assert!(allows_by_address(&policy, 8443));
    assert!(!allows_by_address(&policy, 443));
    assert!(!allows_by_address(&policy, 80));
    Ok(())
}

#[test]
fn decide_host_denies_an_unlisted_name_no_allow_range_could_admit_on_its_port() -> Result<(), Box<dyn std::error::Error>>
{
    let policy = user_defined(&[], &[], &["203.0.113.7:8443"], &[])?;
    let name = Host::new("data.attacker.example")?;
    assert_eq!(decide_host(&policy, &name, 443), Decision::Deny);
    assert_eq!(decide_host(&policy, &name, 8443), Decision::Allow);
    Ok(())
}

fn port(n: u16) -> Result<Port, Box<dyn std::error::Error>> {
    Ok(Port::new(n)?)
}

#[test]
fn amended_adds_an_exact_pass_through_rule_that_then_matches() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["allowed.test"], &[], &[], &[])?;
    let host = Host::new("new.test")?;
    let next = amended(&policy, &host, port(443)?, Decision::Allow).ok_or("expected a value")?;
    assert_eq!(
        rule_host(&next, &host, 443),
        Ruling::Allow(Some(DomainRuleAction::PassThrough))
    );
    assert_eq!(rule_host(&next, &host, 8443), Ruling::Unlisted);
    assert_eq!(
        rule_host(&next, &Host::new("allowed.test")?, 443),
        Ruling::Allow(Some(DomainRuleAction::PassThrough))
    );
    Ok(())
}

#[test]
fn amended_adds_an_exact_denied_domain_that_then_wins() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["new.test"], &[], &[], &[])?;
    let host = Host::new("new.test")?;
    let next = amended(&policy, &host, port(443)?, Decision::Deny).ok_or("expected a value")?;
    assert_eq!(rule_host(&next, &host, 443), Ruling::Deny);
    Ok(())
}

#[test]
fn amended_is_none_when_nothing_would_change() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&[], &[], &[], &[])?;
    let host = Host::new("new.test")?;
    let once = amended(&policy, &host, port(443)?, Decision::Allow).ok_or("expected a value")?;
    assert_eq!(amended(&once, &host, port(443)?, Decision::Allow), None);
    let denied = amended(&policy, &host, port(443)?, Decision::Deny).ok_or("expected a value")?;
    assert_eq!(amended(&denied, &host, port(443)?, Decision::Deny), None);
    assert_eq!(
        amended(&NetworkPolicy::AllowAll, &host, port(443)?, Decision::Deny),
        None
    );
    assert_eq!(
        amended(&NetworkPolicy::DenyAll, &host, port(443)?, Decision::Allow),
        None
    );
    Ok(())
}

#[test]
fn decide_with_action_denies_what_rule_leaves_unlisted() -> Result<(), Box<dyn std::error::Error>> {
    let policy = user_defined(&["allowed.test"], &[], &[], &[])?;
    assert_eq!(
        decide_with_action(&policy, Some("unknown.test"), ip4("203.0.113.5")?, 443),
        (Decision::Deny, None)
    );
    Ok(())
}

#[test]
fn only_an_exact_address_counts_as_an_exact_address_allow() -> Result<(), Box<dyn std::error::Error>> {
    let policy = |ranges: &[&str]| -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
        Ok(NetworkPolicy::UserDefined {
            rules: vec![domain_rule("api.example.com")?],
            allow_ranges: ranges.iter().map(|r| AllowRange::new(*r)).collect::<Result<_, _>>()?,
            deny_ranges: vec![],
            denied_domains: vec![],
            opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
            request_rules: vec![],
        })
    };
    let ip = ip4("203.0.113.7")?;
    assert!(address_allowed_exactly(&policy(&["203.0.113.7/32"])?, ip, 443));
    assert!(!address_allowed_exactly(&policy(&["203.0.113.0/24"])?, ip, 443));
    // A bare address is the one-address range.
    assert!(address_allowed_exactly(&policy(&["203.0.113.7"])?, ip, 443));
    // An address with a port allows that address on that port only.
    assert!(address_allowed_exactly(&policy(&["203.0.113.7:443"])?, ip, 443));
    assert!(!address_allowed_exactly(&policy(&["203.0.113.7:8443"])?, ip, 443));
    assert!(!address_allowed_exactly(&policy(&[])?, ip, 443));
    assert!(!address_allowed_exactly(&NetworkPolicy::DenyAll, ip, 443));
    Ok(())
}

#[test]
fn only_allow_is_an_allow_and_only_a_closed_policy_refuses_every_address() -> Result<(), Box<dyn std::error::Error>> {
    assert!(Decision::Allow.is_allow());
    assert!(!Decision::Deny.is_allow());
    let addr = ip4("203.0.113.9")?;
    assert!(refuses_address(&NetworkPolicy::DenyAll, addr, 443));
    assert!(!refuses_address(&NetworkPolicy::AllowAll, addr, 443));
    Ok(())
}
