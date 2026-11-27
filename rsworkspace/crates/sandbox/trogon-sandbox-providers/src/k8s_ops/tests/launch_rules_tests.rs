use trogon_sandbox_contract::NetworkPolicy;

use crate::RunAsUserError;
use crate::k8s_ops::{
    digest_label_value, is_strong_isolation_runtime_class, parse_run_as_user, policy_needs_egress_proxy,
    runtime_class_name,
};

fn is_valid_label_value(value: &str) -> bool {
    value.len() <= 63
        && value.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && value.chars().next().is_none_or(|c| c.is_ascii_alphanumeric())
        && value.chars().last().is_none_or(|c| c.is_ascii_alphanumeric())
}

fn user_defined(extra: &str) -> Result<NetworkPolicy, serde_json::Error> {
    serde_json::from_str(&format!(
        r#"{{"user_defined":{{"rules":[],"allow_ranges":[],"deny_ranges":[]{extra}}}}}"#
    ))
}

#[test]
fn a_sha256_digest_label_is_a_valid_label_value() {
    let digest = format!("sha256:{}", "ab".repeat(32));

    let value = digest_label_value(&digest);

    assert!(is_valid_label_value(&value), "{value}");
    assert_eq!(value, "ab".repeat(32)[..63]);
}

#[test]
fn a_digest_without_an_algorithm_prefix_is_used_as_is() {
    assert_eq!(digest_label_value("abc-123"), "abc123");
}

#[test]
fn only_domain_or_request_rules_need_the_egress_proxy() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!policy_needs_egress_proxy(&NetworkPolicy::AllowAll));
    assert!(!policy_needs_egress_proxy(&NetworkPolicy::DenyAll));
    assert!(!policy_needs_egress_proxy(&user_defined("")?));
    assert!(policy_needs_egress_proxy(&user_defined(
        r#","denied_domains":["evil.example"]"#
    )?));
    assert!(policy_needs_egress_proxy(&user_defined(
        r#","request_rules":[{"action":"deny"}]"#
    )?));
    let with_rule: NetworkPolicy = serde_json::from_str(
        r#"{"user_defined":{"rules":[{"domain":"example.com"}],"allow_ranges":[],"deny_ranges":[]}}"#,
    )?;
    assert!(policy_needs_egress_proxy(&with_rule));
    Ok(())
}

#[test]
fn a_numeric_user_parses_to_a_uid_and_optional_gid() -> Result<(), RunAsUserError> {
    assert_eq!(parse_run_as_user("2000")?, (2000, None));
    assert_eq!(parse_run_as_user("2000:3000")?, (2000, Some(3000)));
    Ok(())
}

#[test]
fn a_user_name_is_not_numeric() {
    assert!(matches!(
        parse_run_as_user("node"),
        Err(RunAsUserError::NotNumeric { user }) if user == "node"
    ));
    assert!(matches!(
        parse_run_as_user("1000:staff"),
        Err(RunAsUserError::NotNumeric { .. })
    ));
}

#[test]
fn root_is_rejected_as_uid_or_gid() {
    assert!(matches!(
        parse_run_as_user("0"),
        Err(RunAsUserError::Root { user }) if user == "0"
    ));
    assert!(matches!(parse_run_as_user("1000:0"), Err(RunAsUserError::Root { .. })));
}

#[test]
fn an_empty_runtime_class_leaves_the_field_unset() {
    assert_eq!(runtime_class_name(""), None);
    assert_eq!(runtime_class_name("kata-fc").as_deref(), Some("kata-fc"));
}

#[test]
fn kata_and_gvisor_are_strong_isolation_and_runc_is_not() {
    assert!(is_strong_isolation_runtime_class("kata-fc"));
    assert!(is_strong_isolation_runtime_class("GVisor"));
    assert!(!is_strong_isolation_runtime_class("runc"));
}
