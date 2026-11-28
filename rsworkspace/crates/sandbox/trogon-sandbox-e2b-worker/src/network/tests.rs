use serde_json::json;
use trogon_sandbox_contract::{
    AllowRange, DomainPattern, DomainRule, DomainRuleAction, IpRange, LeaseError, NetworkPolicy, OpaqueTunnelPolicy,
};

use super::*;
use crate::api::NewSandbox;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn user_defined(rules: Vec<DomainRule>, allow: Vec<IpRange>, deny: Vec<IpRange>) -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules,
        allow_ranges: allow.into_iter().map(AllowRange::from).collect(),
        deny_ranges: deny,
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

fn rule(domain: &str) -> Result<DomainRule, Box<dyn std::error::Error>> {
    Ok(DomainRule {
        domain: DomainPattern::new(domain)?,
        action: DomainRuleAction::PassThrough,
    })
}

#[test]
fn unset_and_allow_all_leave_the_default() -> TestResult {
    assert_eq!(translate_network_policy(None)?, E2bNetwork::default());
    assert_eq!(
        translate_network_policy(Some(&NetworkPolicy::AllowAll))?,
        E2bNetwork::default()
    );
    Ok(())
}

#[test]
fn deny_all_turns_internet_access_off() -> TestResult {
    let network = translate_network_policy(Some(&NetworkPolicy::DenyAll))?;
    assert_eq!(network.allow_internet_access, Some(InternetAccess::Denied));
    assert!(network.network.is_none());
    Ok(())
}

#[test]
fn hosts_and_ranges_become_allow_out_with_a_default_deny() -> TestResult {
    let policy = user_defined(
        vec![rule("api.example.com")?, rule("10.0.0.7")?, rule("api.example.com")?],
        vec![IpRange::new("192.168.0.0/16")?],
        vec![IpRange::new("192.168.9.0/24")?],
    );
    let network = translate_network_policy(Some(&policy))?;
    assert_eq!(network.allow_internet_access, None);
    let rules = network.network.ok_or("rules")?;
    assert_eq!(rules.allow_out, ["10.0.0.7", "192.168.0.0/16", "api.example.com"]);
    assert_eq!(rules.deny_out, ["0.0.0.0/0", "192.168.9.0/24"]);
    assert_eq!(
        serde_json::to_value(&rules)?,
        json!({"allowOut": ["10.0.0.7", "192.168.0.0/16", "api.example.com"], "denyOut": ["0.0.0.0/0", "192.168.9.0/24"]})
    );
    Ok(())
}

#[test]
fn an_empty_allow_list_denies_everything() -> TestResult {
    let network = translate_network_policy(Some(&user_defined(vec![], vec![], vec![])))?;
    assert_eq!(network.allow_internet_access, Some(InternetAccess::Denied));
    Ok(())
}

#[test]
fn what_e2b_cannot_express_is_refused() -> TestResult {
    let with_port = user_defined(vec![rule("example.com:8443")?], vec![], vec![]);
    assert!(matches!(
        translate_network_policy(Some(&with_port)),
        Err(LeaseError::InvalidNetworkPolicy { .. })
    ));
    let refuse = NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::Refuse,
        request_rules: vec![],
    };
    assert!(translate_network_policy(Some(&refuse)).is_err());
    let forwarded: DomainRule = serde_json::from_value(json!({
        "domain": "example.com",
        "action": {"forward": {"forward_url": "https://relay.example.com/in"}}
    }))?;
    assert!(translate_network_policy(Some(&user_defined(vec![forwarded], vec![], vec![]))).is_err());
    let denied: NetworkPolicy = serde_json::from_value(json!({
        "user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "opaque_tunnels": "allow", "denied_domains": ["bad.example.com"]}
    }))?;
    assert!(translate_network_policy(Some(&denied)).is_err());
    let requests: NetworkPolicy = serde_json::from_value(json!({
        "user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "opaque_tunnels": "allow", "request_rules": [{"action": "deny", "domains": ["example.com"]}]}
    }))?;
    assert!(translate_network_policy(Some(&requests)).is_err());
    let scoped: NetworkPolicy = serde_json::from_value(json!({
        "user_defined": {"rules": [], "allow_ranges": ["10.0.0.5:8080"], "deny_ranges": [], "opaque_tunnels": "allow"}
    }))?;
    assert!(matches!(
        translate_network_policy(Some(&scoped)),
        Err(LeaseError::InvalidNetworkPolicy { .. })
    ));
    Ok(())
}

#[test]
fn a_single_address_allow_range_is_sent_as_a_one_address_block() -> TestResult {
    let policy: NetworkPolicy = serde_json::from_value(json!({
        "user_defined": {"rules": [], "allow_ranges": ["10.0.0.6"], "deny_ranges": [], "opaque_tunnels": "allow"}
    }))?;
    let network = translate_network_policy(Some(&policy))?
        .network
        .ok_or("network rules")?;
    assert_eq!(network.allow_out, vec!["10.0.0.6/32".to_owned()]);
    Ok(())
}

#[test]
fn internet_access_serializes_as_the_provider_flag() -> TestResult {
    assert_eq!(serde_json::to_value(InternetAccess::Allowed)?, json!(true));
    assert_eq!(serde_json::to_value(InternetAccess::Denied)?, json!(false));
    let sandbox = NewSandbox {
        template_id: "base".to_owned(),
        timeout: 60,
        allow_internet_access: Some(InternetAccess::Denied),
        ..NewSandbox::default()
    };
    assert_eq!(
        serde_json::to_value(&sandbox)?,
        json!({"templateID": "base", "timeout": 60, "allow_internet_access": false})
    );
    Ok(())
}

#[test]
fn a_policy_that_does_not_opt_in_to_opaque_tunnels_is_refused() -> TestResult {
    let silent: NetworkPolicy = serde_json::from_value(json!({
        "user_defined": {"rules": [{"domain": "example.com"}], "allow_ranges": [], "deny_ranges": []}
    }))?;
    assert!(matches!(
        translate_network_policy(Some(&silent)),
        Err(LeaseError::InvalidNetworkPolicy { .. })
    ));
    Ok(())
}
