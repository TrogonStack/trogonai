use trogon_sandbox_contract::NetworkPolicy;

use super::{network_mode, requires_tls_intercept};
use crate::EgressPolicy;

fn user_defined(rule_json: &str) -> Result<NetworkPolicy, serde_json::Error> {
    serde_json::from_str(&format!(
        r#"{{"user_defined":{{"rules":[{rule_json}],"allow_ranges":[],"deny_ranges":[]}}}}"#
    ))
}

#[test]
fn only_transform_and_forward_rules_intercept_tls() -> Result<(), serde_json::Error> {
    assert!(!requires_tls_intercept(&NetworkPolicy::AllowAll));
    assert!(!requires_tls_intercept(&NetworkPolicy::DenyAll));
    assert!(!requires_tls_intercept(&user_defined(r#"{"domain":"a.example.com"}"#)?));
    assert!(requires_tls_intercept(&user_defined(
        r#"{"domain":"a.example.com","action":{"forward":{"forward_url":"https://f.example.com/x"}}}"#
    )?));
    Ok(())
}

#[test]
fn network_policy_decides_the_mode_before_the_egress_policy() -> Result<(), serde_json::Error> {
    let allow_hosts = EgressPolicy::AllowHosts(vec![]);
    assert_eq!(network_mode(Some(&NetworkPolicy::DenyAll), &allow_hosts), "none");
    assert_eq!(
        network_mode(Some(&NetworkPolicy::AllowAll), &EgressPolicy::DenyAll),
        "bridge"
    );
    assert_eq!(
        network_mode(
            Some(&user_defined(r#"{"domain":"a.example.com"}"#)?),
            &EgressPolicy::DenyAll
        ),
        "bridge"
    );
    assert_eq!(network_mode(None, &EgressPolicy::DenyAll), "none");
    assert_eq!(network_mode(None, &allow_hosts), "bridge");
    Ok(())
}
