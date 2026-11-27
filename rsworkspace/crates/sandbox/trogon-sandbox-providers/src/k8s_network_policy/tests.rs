use std::collections::BTreeMap;
use std::error::Error;

use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform, DomainRule,
    DomainRuleAction, HeaderName, LeaseId, PullSecretId,
};

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn labels() -> BTreeMap<String, String> {
    BTreeMap::from([("app.kubernetes.io/name".to_string(), "sandbox-agent".to_string())])
}

fn range(text: &str) -> Result<IpRange, Box<dyn Error>> {
    Ok(IpRange::new(text)?)
}

fn user_defined(
    rules: Vec<DomainRule>,
    allow_ranges: Vec<IpRange>,
    deny_ranges: Vec<IpRange>,
) -> ContractNetworkPolicy {
    ContractNetworkPolicy::UserDefined {
        rules,
        allow_ranges: allow_ranges.into_iter().map(AllowRange::from).collect(),
        deny_ranges,
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

fn pass_through_rule() -> Result<DomainRule, Box<dyn Error>> {
    Ok(DomainRule {
        domain: "example.com".parse()?,
        action: DomainRuleAction::PassThrough,
    })
}

fn transform_rule() -> Result<DomainRule, Box<dyn Error>> {
    Ok(DomainRule {
        domain: "example.com".parse()?,
        action: DomainRuleAction::Transform(CredentialTransform {
            match_rules: vec![],
            credential: CredentialRequest {
                provider: CredentialProvider::Registry,
                pull_secret_id: PullSecretId::new("test-secret"),
                scope: CredentialScope::RegistryPull {
                    repository_glob: "*".to_string(),
                },
                ttl_seconds: 60,
                requester: None,
            },
            injection: CredentialInjection::Header(HeaderName::new("authorization")?),
        }),
    })
}

fn first_ip_block(policy: &NetworkPolicy) -> Result<IPBlock, Box<dyn Error>> {
    let spec = policy.spec.as_ref().ok_or("spec")?;
    let egress = spec.egress.as_ref().ok_or("egress")?;
    let to = egress.first().ok_or("rule")?.to.as_ref().ok_or("to")?;
    Ok(to.first().ok_or("peer")?.ip_block.clone().ok_or("ip_block")?)
}

fn manifest(policy: &ContractNetworkPolicy) -> Result<NetworkPolicy, Box<dyn Error>> {
    Ok(build_network_policy_manifest(&labels(), policy, "default", "job-1").ok_or("manifest")?)
}

#[test]
fn allow_all_returns_none() {
    assert!(
        build_network_policy_manifest(
            &labels(),
            &ContractNetworkPolicy::AllowAll,
            "default",
            "rs-acme-widget-abc12345"
        )
        .is_none()
    );
}

#[test]
fn deny_all_returns_egress_only_policy_with_empty_rules() -> TestResult {
    let result = build_network_policy_manifest(
        &labels(),
        &ContractNetworkPolicy::DenyAll,
        "default",
        "rs-acme-widget-abc12345",
    )
    .ok_or("DenyAll must produce a NetworkPolicy")?;

    let spec = result.spec.ok_or("spec")?;
    assert_eq!(spec.policy_types, Some(vec!["Egress".to_string()]));
    assert_eq!(spec.egress, Some(vec![]));
    assert_eq!(spec.pod_selector.ok_or("selector")?.match_labels, Some(labels()));
    assert_eq!(result.metadata.name.as_deref(), Some("rs-acme-widget-abc12345-netpol"));
    assert_eq!(result.metadata.namespace.as_deref(), Some("default"));
    Ok(())
}

#[test]
fn derived_names_follow_the_job_name() {
    assert_eq!(network_policy_name("job"), "job-netpol");
    assert_eq!(egress_proxy_configmap_name("job"), "job-egress-policy");
}

#[test]
fn user_defined_deny_range_that_is_genuine_subset_is_excepted() -> TestResult {
    let policy = user_defined(vec![], vec![range("10.0.0.0/8")?], vec![range("10.1.2.0/24")?]);
    let block = first_ip_block(&manifest(&policy)?)?;
    assert_eq!(block.cidr, "10.0.0.0/8");
    assert_eq!(block.except, Some(vec!["10.1.2.0/24".to_string()]));
    Ok(())
}

#[test]
fn user_defined_deny_range_that_is_not_subset_is_not_excepted() -> TestResult {
    let policy = user_defined(vec![], vec![range("10.1.2.0/24")?], vec![range("10.0.0.0/8")?]);
    assert_eq!(first_ip_block(&manifest(&policy)?)?.except, None);
    Ok(())
}

#[test]
fn user_defined_disjoint_deny_range_is_not_excepted() -> TestResult {
    let policy = user_defined(vec![], vec![range("10.0.0.0/24")?], vec![range("192.168.0.0/24")?]);
    assert_eq!(first_ip_block(&manifest(&policy)?)?.except, None);
    Ok(())
}

#[test]
fn user_defined_ipv6_ranges_are_carved_out_too() -> TestResult {
    let policy = user_defined(vec![], vec![range("2001:db8::/32")?], vec![range("2001:db8:1::/48")?]);
    assert_eq!(
        first_ip_block(&manifest(&policy)?)?.except,
        Some(vec!["2001:db8:1::/48".to_string()])
    );
    Ok(())
}

#[test]
fn an_allow_range_with_a_port_allows_tcp_to_that_port_only() -> TestResult {
    let policy = ContractNetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("10.0.0.5:8080")?, AllowRange::new("10.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let egress = manifest(&policy)?.spec.ok_or("spec")?.egress.ok_or("egress")?;
    let scoped = egress.first().ok_or("scoped rule")?;
    let peer = scoped.to.as_ref().and_then(|to| to.first()).ok_or("peer")?;
    assert_eq!(peer.ip_block.as_ref().ok_or("ip_block")?.cidr, "10.0.0.5/32");
    assert_eq!(
        scoped.ports,
        Some(vec![NetworkPolicyPort {
            port: Some(IntOrString::Int(8080)),
            protocol: Some("TCP".to_string()),
            ..Default::default()
        }])
    );
    assert_eq!(egress.get(1).ok_or("unscoped rule")?.ports, None);
    Ok(())
}

#[test]
fn user_defined_empty_everything_still_denies_all_egress() -> TestResult {
    let spec = manifest(&user_defined(vec![], vec![], vec![]))?.spec.ok_or("spec")?;
    assert_eq!(spec.egress, Some(vec![]));
    Ok(())
}

#[test]
fn user_defined_with_domain_rules_still_builds_cidr_network_policy() -> TestResult {
    let policy = user_defined(vec![pass_through_rule()?], vec![range("10.0.0.0/8")?], vec![]);
    let spec = manifest(&policy)?.spec.ok_or("spec")?;
    assert_eq!(spec.egress.ok_or("egress")?.len(), 1);
    Ok(())
}

#[test]
fn build_egress_sidecar_sets_env_vars_and_iptables_command() -> TestResult {
    let policy = user_defined(vec![transform_rule()?], vec![], vec![]);
    let resources = build_egress_sidecar(&policy, "job-1", None)?;

    assert_eq!(resources.sidecar_container.name, "egress-proxy");
    assert_eq!(resources.init_container.name, "egress-proxy-iptables-init");
    let env = resources.sidecar_container.env.as_ref().ok_or("env")?;
    let value_of = |name: &str| {
        env.iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.value.as_deref())
    };
    assert_eq!(value_of("EGRESS_PROXY_LISTEN_ADDR"), Some("0.0.0.0:3128"));
    assert_eq!(
        value_of("EGRESS_PROXY_POLICY_FILE"),
        Some("/etc/sandbox-egress-proxy/policy.json")
    );
    assert_eq!(
        value_of("EGRESS_PROXY_CA_OUT_FILE"),
        Some("/etc/sandbox-egress-proxy-ca/ca.pem")
    );
    assert_eq!(value_of("EGRESS_PROXY_EXEMPT_UID"), Some("65531"));

    let ca_volume = resources
        .ca_volume
        .as_ref()
        .ok_or("Transform rule must produce a ca_volume")?;
    assert_eq!(ca_volume.name, EGRESS_PROXY_CA_VOLUME_NAME);
    assert!(ca_volume.empty_dir.is_some());
    let mounts = resources.sidecar_container.volume_mounts.as_ref().ok_or("mounts")?;
    assert!(
        mounts
            .iter()
            .any(|mount| mount.name == EGRESS_PROXY_CA_VOLUME_NAME && mount.mount_path == EGRESS_PROXY_CA_MOUNT_PATH)
    );
    assert!(mounts.iter().any(|mount| mount.read_only == Some(true)));

    let sidecar_context = resources.sidecar_container.security_context.as_ref().ok_or("context")?;
    assert_eq!(sidecar_context.run_as_user, Some(65531));
    assert_eq!(sidecar_context.run_as_non_root, Some(true));
    assert_eq!(sidecar_context.allow_privilege_escalation, Some(false));
    assert_eq!(sidecar_context.read_only_root_filesystem, Some(true));
    assert_eq!(
        sidecar_context.capabilities.as_ref().ok_or("capabilities")?.drop,
        Some(vec!["ALL".to_string()])
    );

    let init_context = resources.init_container.security_context.as_ref().ok_or("context")?;
    assert_eq!(init_context.run_as_user, Some(0));
    assert_eq!(init_context.allow_privilege_escalation, Some(false));
    assert_eq!(init_context.read_only_root_filesystem, Some(true));
    assert_eq!(
        init_context.capabilities.as_ref().ok_or("capabilities")?.add,
        Some(vec!["NET_ADMIN".to_string()])
    );
    let args = resources.init_container.args.as_ref().ok_or("args")?;
    assert!(args[0].contains("iptables -t nat -A OUTPUT"));
    assert!(args[0].contains("--to-port 3128"));
    assert!(args[0].contains("-m owner --uid-owner 65531 -j RETURN"));
    assert!(!args[0].contains("mark"));

    let data = resources.config_map.data.as_ref().ok_or("data")?;
    assert!(data.contains_key(EGRESS_PROXY_CONFIGMAP_KEY));
    assert_eq!(
        resources.config_map.metadata.name.as_deref(),
        Some("job-1-egress-policy")
    );
    assert_eq!(resources.volume.name, "egress-proxy-policy");
    Ok(())
}

#[test]
fn build_egress_sidecar_with_only_pass_through_rules_omits_ca_wiring() -> TestResult {
    let policy = user_defined(vec![pass_through_rule()?], vec![], vec![]);
    let resources = build_egress_sidecar(&policy, "job-1", None)?;

    assert!(resources.ca_volume.is_none());
    let env = resources.sidecar_container.env.as_ref().ok_or("env")?;
    assert!(!env.iter().any(|entry| entry.name == "EGRESS_PROXY_CA_OUT_FILE"));
    let mounts = resources.sidecar_container.volume_mounts.as_ref().ok_or("mounts")?;
    assert!(!mounts.iter().any(|mount| mount.name == EGRESS_PROXY_CA_VOLUME_NAME));
    Ok(())
}

#[test]
fn build_egress_sidecar_for_policies_without_domain_rules_omits_ca_wiring() -> TestResult {
    for policy in [ContractNetworkPolicy::AllowAll, ContractNetworkPolicy::DenyAll] {
        assert!(build_egress_sidecar(&policy, "job-1", None)?.ca_volume.is_none());
    }
    Ok(())
}

#[test]
fn build_egress_sidecar_forwards_violation_event_settings() -> TestResult {
    let policy = user_defined(vec![pass_through_rule()?], vec![], vec![]);
    let events = EgressViolationEvents::new("nats://127.0.0.1:4222", LeaseId::new("lease-1"))?;
    let resources = build_egress_sidecar(&policy, "job-1", Some(&events))?;

    let env = resources.sidecar_container.env.as_ref().ok_or("env")?;
    for (name, value) in events.env_pairs() {
        assert!(
            env.iter()
                .any(|entry| entry.name == name && entry.value.as_deref() == Some(value))
        );
    }
    Ok(())
}
