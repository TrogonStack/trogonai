use std::collections::BTreeMap;

use serde_json::{Value, json};
use trogon_sandbox_contract::{
    EgressEnforcementLevel, IsolationTier, LeaseError, NetworkPolicy, ProviderKind, SandboxSpec,
};
use trogon_sandbox_protocol::ProviderError;
use trogon_sandbox_vercel_worker::{
    DEFAULT_MAX_VCPUS, KNOWN_RUNTIMES, LeaseSandbox, NATIVE_OPTION_KEYS, NewSandbox, PROFILE, Resources,
    VercelApiError, VercelConfig, VercelEndpoint, VercelEndpointError, VercelPlan, VercelProjectId,
    VercelProviderError, VercelTeamId, VercelToken, VercelTokenError, check_runtime, profile_with_resources, resources,
    resources_for, translate_network_policy,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn spec(vcpu_millis: Option<u32>, memory_mib: u32, disk_mib: Option<u32>) -> Result<SandboxSpec, serde_json::Error> {
    let mut wire = json!({
        "image": {"kind": "registry", "reference": {"registry": "", "repository": "node24"}},
        "workspace": {"empty": {}},
        "cpu_seconds": 10,
        "memory_mib": memory_mib,
    });
    if let Some(millis) = vcpu_millis {
        wire["vcpu_millis"] = json!(millis);
    }
    if let Some(disk) = disk_mib {
        wire["disk_mib"] = json!(disk);
    }
    serde_json::from_value(wire)
}

fn policy(change: impl FnOnce(&mut Value)) -> Result<NetworkPolicy, serde_json::Error> {
    // The vendor firewall cannot refuse opaque tunnels, so every policy it serves has to opt in.
    let mut wire =
        json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": [], "opaque_tunnels": "allow"}});
    change(&mut wire["user_defined"]);
    serde_json::from_value(wire)
}

fn translated(policy: &NetworkPolicy) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    Ok(translate_network_policy(Some(policy))?)
}

fn is_invalid_policy(policy: &NetworkPolicy) -> bool {
    matches!(
        translate_network_policy(Some(policy)),
        Err(LeaseError::InvalidNetworkPolicy { .. })
    )
}

#[test]
fn a_known_runtime_is_accepted_and_any_other_is_an_invalid_spec() {
    for runtime in KNOWN_RUNTIMES {
        assert!(check_runtime(runtime).is_ok(), "{runtime}");
    }
    assert!(matches!(check_runtime("node18"), Err(LeaseError::InvalidSpec { .. })));
    assert_eq!(NATIVE_OPTION_KEYS, ["persistent"]);
}

#[test]
fn unset_and_allow_all_leave_the_vendor_default() -> TestResult {
    assert_eq!(translate_network_policy(None)?, None);
    assert_eq!(translated(&NetworkPolicy::AllowAll)?, None);
    Ok(())
}

#[test]
fn deny_all_and_an_empty_allow_list_are_deny_all() -> TestResult {
    let deny = Some(json!({"mode": "deny-all"}));
    assert_eq!(translated(&NetworkPolicy::DenyAll)?, deny);
    assert_eq!(translated(&policy(|_| {})?)?, deny);
    Ok(())
}

#[test]
fn hosts_and_ranges_become_sorted_allow_lists() -> TestResult {
    let policy = policy(|wire| {
        wire["rules"] = json!([{"domain": "b.example.com"}, {"domain": "a.example.com"}, {"domain": "a.example.com"}]);
        wire["allow_ranges"] = json!(["192.168.0.0/16", "10.0.0.0/8", "10.0.0.0/8"]);
        wire["deny_ranges"] = json!(["192.168.9.0/24"]);
    })?;
    assert_eq!(
        translated(&policy)?,
        Some(json!({
            "allow": ["a.example.com", "b.example.com"],
            "subnets": {"allow": ["10.0.0.0/8", "192.168.0.0/16"], "deny": ["192.168.9.0/24"]}
        }))
    );
    Ok(())
}

#[test]
fn empty_parts_are_left_out() -> TestResult {
    let hosts_only = policy(|wire| wire["rules"] = json!([{"domain": "a.example.com"}]))?;
    assert_eq!(translated(&hosts_only)?, Some(json!({"allow": ["a.example.com"]})));
    let ranges_only = policy(|wire| wire["allow_ranges"] = json!(["10.0.0.0/8"]))?;
    assert_eq!(
        translated(&ranges_only)?,
        Some(json!({"subnets": {"allow": ["10.0.0.0/8"]}}))
    );
    Ok(())
}

#[test]
fn what_vercel_cannot_express_is_refused() -> TestResult {
    let with_port = policy(|wire| wire["rules"] = json!([{"domain": "example.com:8443"}]))?;
    assert!(is_invalid_policy(&with_port));
    let refuse_tunnels = policy(|wire| wire["opaque_tunnels"] = json!("refuse"))?;
    assert!(is_invalid_policy(&refuse_tunnels));
    let denied = policy(|wire| wire["denied_domains"] = json!(["x.com"]))?;
    assert!(is_invalid_policy(&denied));
    let request_rules = policy(|wire| wire["request_rules"] = json!([{"action": "deny"}]))?;
    assert!(is_invalid_policy(&request_rules));
    let forwarded = policy(|wire| {
        wire["rules"] = json!([{
            "domain": "example.com",
            "action": {"forward": {"forward_url": "https://forward.example.com/"}}
        }]);
    })?;
    assert!(is_invalid_policy(&forwarded));
    let scoped = policy(|wire| wire["allow_ranges"] = json!(["10.0.0.5:8080"]))?;
    assert!(is_invalid_policy(&scoped));
    Ok(())
}

#[test]
fn a_single_address_allow_range_is_sent_as_a_one_address_subnet() -> TestResult {
    let single = policy(|wire| wire["allow_ranges"] = json!(["10.0.0.6"]))?;
    assert_eq!(
        translated(&single)?,
        Some(json!({"subnets": {"allow": ["10.0.0.6/32"]}}))
    );
    Ok(())
}

#[test]
fn the_profile_names_vercel_and_a_microvm() {
    assert_eq!(PROFILE.provider, ProviderKind::Vercel);
    assert_eq!(PROFILE.isolation.default_tier, IsolationTier::MicroVm);
    assert_eq!(PROFILE.isolation.egress_enforcement, EgressEnforcementLevel::BestEffort);
    assert_eq!(PROFILE.lifecycle.max_lease_lifetime_seconds, Some(86_400));
}

#[test]
fn egress_takes_hosts_and_cidr_blocks_and_ports_are_declared_at_create() {
    let egress = PROFILE.isolation.egress;
    assert!(egress.deny_all.is_supported() && egress.host_allowlist.is_supported());
    assert!(egress.cidr_allowlist.is_supported());
    assert!(!egress.proxy_mediated.is_supported());
    assert!(PROFILE.isolation.supports_port_preview.is_supported());
    assert!(!PROFILE.isolation.supports_expose_after_create.is_supported());
}

#[test]
fn pause_resume_and_native_snapshots_are_declared_and_oci_export_is_not() {
    assert!(PROFILE.lifecycle.supports_pause_resume.is_supported());
    assert!(PROFILE.snapshot.supports_native_snapshots.is_supported());
    assert!(!PROFILE.snapshot.can_snapshot.is_supported());
}

#[test]
fn the_size_range_follows_the_configured_ceiling_and_has_no_disk() -> TestResult {
    let default_range = PROFILE.resources.vcpu_millis.ok_or("range")?;
    assert_eq!(
        (default_range.min(), default_range.max()),
        (1000, DEFAULT_MAX_VCPUS * 1000)
    );
    assert!(PROFILE.resources.disk_mib.is_none());
    let wide = profile_with_resources(resources(16));
    assert_eq!(wide.resources.vcpu_millis.ok_or("range")?.max(), 16_000);
    assert_eq!(wide.provider, ProviderKind::Vercel);
    assert_eq!(resources(0).vcpu_millis.ok_or("range")?.max(), 1000);
    Ok(())
}

#[test]
fn no_size_leaves_the_vendor_default() -> TestResult {
    assert_eq!(resources_for(&spec(None, 512, None)?, 8)?, None);
    assert_eq!(resources_for(&spec(None, 2048, None)?, 8)?, None);
    Ok(())
}

#[test]
fn cpu_rounds_up_to_whole_vcpus() -> TestResult {
    for (millis, vcpus) in [(500, 1), (1000, 1), (1001, 2), (8000, 8)] {
        assert_eq!(
            resources_for(&spec(Some(millis), 512, None)?, 8)?,
            Some(Resources { vcpus }),
            "{millis}"
        );
    }
    Ok(())
}

#[test]
fn memory_lifts_the_vcpu_count() -> TestResult {
    for (cpu, memory, vcpus) in [
        (None, 4096, 2),
        (None, 4097, 3),
        (Some(1000), 6000, 3),
        (Some(4000), 1024, 4),
    ] {
        assert_eq!(
            resources_for(&spec(cpu, memory, None)?, 8)?,
            Some(Resources { vcpus }),
            "{cpu:?} {memory}"
        );
    }
    assert_eq!(serde_json::to_value(Resources { vcpus: 2 })?, json!({"vcpus": 2}));
    Ok(())
}

#[test]
fn a_size_above_the_limit_or_a_disk_size_is_an_invalid_spec() -> TestResult {
    for spec in [
        spec(Some(9000), 512, None)?,
        spec(None, 20_000, None)?,
        spec(None, 512, Some(100))?,
    ] {
        assert!(matches!(resources_for(&spec, 8), Err(LeaseError::InvalidSpec { .. })));
    }
    Ok(())
}

#[test]
fn plans_parse_case_insensitively_and_cap_session_and_size() -> TestResult {
    assert_eq!("Hobby".parse::<VercelPlan>()?, VercelPlan::Hobby);
    assert_eq!("pro".parse::<VercelPlan>()?, VercelPlan::Pro);
    assert_eq!("ENTERPRISE".parse::<VercelPlan>()?, VercelPlan::Enterprise);
    assert_eq!(VercelPlan::default(), VercelPlan::Pro);
    let unknown = "team".parse::<VercelPlan>().err().ok_or("accepted")?;
    assert_eq!(unknown.name(), "team");
    assert_eq!(VercelPlan::Hobby.max_session_seconds(), 2700);
    assert_eq!(VercelPlan::Pro.max_session_seconds(), 86_400);
    assert_eq!(VercelPlan::Enterprise.max_session_seconds(), 86_400);
    assert_eq!(
        [VercelPlan::Hobby, VercelPlan::Pro, VercelPlan::Enterprise].map(VercelPlan::max_vcpus),
        [4, 8, 32]
    );
    for plan in [VercelPlan::Hobby, VercelPlan::Pro, VercelPlan::Enterprise] {
        assert_eq!(plan.to_string().parse::<VercelPlan>()?, plan);
    }
    Ok(())
}

#[test]
fn a_lifetime_past_the_plan_cap_is_refused() {
    assert!(VercelPlan::Hobby.check_lifetime(2700).is_ok());
    assert!(matches!(
        VercelPlan::Hobby.check_lifetime(2701),
        Err(LeaseError::WouldExceedMaxLifetime { max: 2700 })
    ));
    assert!(VercelPlan::Pro.check_lifetime(86_400).is_ok());
}

#[test]
fn an_endpoint_is_https_or_loopback_and_plain() -> TestResult {
    assert_eq!(
        VercelEndpoint::parse("https://vercel.com/api")?.as_str(),
        "https://vercel.com/api"
    );
    assert_eq!(
        "http://127.0.0.1:9".parse::<VercelEndpoint>()?.as_str(),
        "http://127.0.0.1:9/"
    );
    assert!(VercelEndpoint::parse("http://localhost:1").is_ok());
    assert_eq!(
        VercelEndpoint::parse("http://example.com").err(),
        Some(VercelEndpointError::InsecureScheme)
    );
    assert_eq!(
        VercelEndpoint::parse("ftp://127.0.0.1").err(),
        Some(VercelEndpointError::InsecureScheme)
    );
    for url in [
        "https://u:p@example.com",
        "https://example.com/?q=1",
        "https://example.com/#f",
    ] {
        assert_eq!(
            VercelEndpoint::parse(url).err(),
            Some(VercelEndpointError::NotPlain),
            "{url}"
        );
    }
    assert!(matches!(
        VercelEndpoint::parse("nope"),
        Err(VercelEndpointError::Unparsable(_))
    ));
    Ok(())
}

#[test]
fn a_token_is_required_header_safe_and_never_printed() -> TestResult {
    assert_eq!(VercelToken::new("").err(), Some(VercelTokenError::Empty));
    assert_eq!(
        "bad\nvalue".parse::<VercelToken>().err(),
        Some(VercelTokenError::NotAHeader)
    );
    let token = "secret-value".parse::<VercelToken>()?;
    assert_eq!(format!("{token:?}"), "VercelToken(..)");
    Ok(())
}

#[test]
fn team_and_project_ids_must_not_be_empty() -> TestResult {
    assert_eq!(VercelTeamId::new("team_1")?.as_str(), "team_1");
    assert_eq!("team_2".parse::<VercelTeamId>()?.as_str(), "team_2");
    assert!(VercelTeamId::new("").is_err());
    assert_eq!(VercelProjectId::new("prj_1")?.as_str(), "prj_1");
    assert_eq!("prj_2".parse::<VercelProjectId>()?.as_str(), "prj_2");
    assert!("".parse::<VercelProjectId>().is_err());
    Ok(())
}

#[test]
fn a_config_hands_back_what_it_was_built_from() -> TestResult {
    let config = VercelConfig::new(
        VercelEndpoint::parse("https://vercel.com/api")?,
        VercelTeamId::new("team_1")?,
        VercelProjectId::new("prj_1")?,
        VercelToken::new("t")?,
    );
    assert_eq!(config.endpoint().as_str(), "https://vercel.com/api");
    assert_eq!(config.team_id().as_str(), "team_1");
    assert_eq!(config.project_id().as_str(), "prj_1");
    assert!(format!("{config:?}").contains("VercelToken(..)"));
    Ok(())
}

#[test]
fn a_new_sandbox_serializes_only_what_is_set() -> TestResult {
    let mut sandbox = NewSandbox::new(VercelProjectId::new("prj_1")?, 60_000, true);
    assert_eq!(
        serde_json::to_value(&sandbox)?,
        json!({"projectId": "prj_1", "timeout": 60_000, "persistent": true})
    );
    sandbox.runtime = Some("node24".to_owned());
    sandbox.image = Some("img".to_owned());
    sandbox.source = Some(json!({"type": "git"}));
    sandbox.ports = vec![3000];
    sandbox.resources = Some(Resources { vcpus: 2 });
    sandbox.network_policy = Some(json!({"mode": "deny-all"}));
    sandbox.env = BTreeMap::from([("A".to_owned(), "b".to_owned())]);
    sandbox.region = Some("iad1".to_owned());
    sandbox.tags = BTreeMap::from([("k".to_owned(), "v".to_owned())]);
    assert_eq!(
        serde_json::to_value(&sandbox)?,
        json!({
            "projectId": "prj_1",
            "runtime": "node24",
            "image": "img",
            "source": {"type": "git"},
            "timeout": 60_000,
            "ports": [3000],
            "resources": {"vcpus": 2},
            "persistent": true,
            "networkPolicy": {"mode": "deny-all"},
            "env": {"A": "b"},
            "region": "iad1",
            "tags": {"k": "v"}
        })
    );
    Ok(())
}

#[test]
fn a_lease_sandbox_names_the_sandbox_session_and_routes() {
    let sandbox = LeaseSandbox {
        name: "sbx-a".to_owned(),
        session_id: "sess_1".to_owned(),
        routes: BTreeMap::from([(3000, "https://one.example.test".to_owned())]),
    };
    assert_eq!(sandbox.clone(), sandbox);
    assert_eq!(
        sandbox.routes.get(&3000).map(String::as_str),
        Some("https://one.example.test")
    );
}

#[test]
fn provider_errors_become_backend_errors_unless_the_protocol_names_them() {
    let lease: ProviderError = VercelProviderError::from(LeaseError::Backend {
        message: "x".to_owned(),
    })
    .into();
    assert!(matches!(lease, ProviderError::Backend(_)));
    let api: ProviderError = VercelProviderError::from(VercelApiError::Transport("x".to_owned())).into();
    assert!(matches!(api, ProviderError::Backend(_)));
    let mounts: ProviderError = VercelProviderError::MountsUnsupported.into();
    assert!(matches!(mounts, ProviderError::Backend(_)));
}

#[test]
fn a_policy_that_does_not_opt_in_to_opaque_tunnels_is_refused() -> TestResult {
    let silent = policy(|wire| {
        if let Some(wire) = wire.as_object_mut() {
            wire.remove("opaque_tunnels");
        }
    })?;
    assert!(is_invalid_policy(&silent), "{silent:?}");
    Ok(())
}
