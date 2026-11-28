use std::error::Error;
use std::path::Path;

use serde_json::json;
use trogon_sandbox_contract::{LeaseId, LeaseRequest, NetworkPolicy};

use super::{EgressPolicy, EgressRoute, build_agent_env, ensure_network_policy_fits_backend, map_egress_route};
use crate::config::IsolationBackendKind;
use crate::lease_proxy::LeaseProxyToken;
use crate::proxy_env::{CaCertificate, ProxyCredentials, ProxyEndpoint, ProxyEnvironment};
use crate::tool_proxy_env::ToolProxyEnv;

fn lease_request(env: &[(&str, &str)]) -> Result<LeaseRequest, serde_json::Error> {
    let env: serde_json::Map<String, serde_json::Value> =
        env.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect();
    serde_json::from_value(json!({
        "tenant": "t",
        "provider_pref": ["subprocess"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "registry.example", "repository": "img", "tag": "latest" },
            },
            "workspace": { "empty": {} },
            "cpu_seconds": 2,
            "memory_mib": 512,
            "env": env,
            "mounts": [],
        },
        "ttl_seconds": 300,
        "idempotency_key": "k",
    }))
}

fn token() -> String {
    format!("tok{}", 123)
}

fn proxy_environment(with_ca: bool) -> Result<ProxyEnvironment, Box<dyn Error>> {
    Ok(ProxyEnvironment::new(
        ProxyEndpoint::relay(),
        ProxyCredentials::new(LeaseProxyToken::parse(token())?),
        with_ca.then(|| CaCertificate::new("/host/ca.pem".into())),
    ))
}

fn value<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

fn user_defined() -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

#[test]
fn agent_env_without_a_proxy_sets_no_proxy_variables() -> Result<(), Box<dyn Error>> {
    let env = build_agent_env(&lease_request(&[])?, &LeaseId::new("l"), Path::new("/w"), None);
    assert!(value(&env, "HTTPS_PROXY").is_none());
    assert!(value(&env, "SSL_CERT_FILE").is_none());
    Ok(())
}

#[test]
fn a_payload_java_tool_options_follows_ours_so_its_properties_win() -> Result<(), Box<dyn Error>> {
    let jar = tempfile::NamedTempFile::new()?;
    let proxy = proxy_environment(false)?.with_tools(ToolProxyEnv::enabled(Some(jar.path().to_path_buf())));
    let env = build_agent_env(
        &lease_request(&[("JAVA_TOOL_OPTIONS", "-Dhttps.proxyPort=9")])?,
        &LeaseId::new("l"),
        Path::new("/w"),
        Some(&proxy),
    );
    let options = value(&env, "JAVA_TOOL_OPTIONS").ok_or("JAVA_TOOL_OPTIONS is set")?;
    assert!(options.starts_with("-javaagent:"));
    assert!(options.contains(" -Dhttps.proxyPort=9"));
    assert_eq!(env.iter().filter(|(n, _)| n == "JAVA_TOOL_OPTIONS").count(), 1);
    Ok(())
}

#[test]
fn agent_env_with_a_proxy_wins_over_the_lease_env() -> Result<(), Box<dyn Error>> {
    let proxy = proxy_environment(true)?;
    let env = build_agent_env(
        &lease_request(&[("HTTPS_PROXY", "http://elsewhere:1"), ("KEEP", "1")])?,
        &LeaseId::new("l"),
        Path::new("/w"),
        Some(&proxy),
    );
    assert_eq!(
        value(&env, "HTTPS_PROXY"),
        Some(format!("http://sandbox:{}@localhost:3128", token()).as_str())
    );
    assert_eq!(value(&env, "NO_PROXY"), Some(""));
    assert_eq!(
        value(&env, "SSL_CERT_FILE"),
        Some("/etc/sandbox-egress-proxy-ca/ca.pem")
    );
    assert_eq!(value(&env, "KEEP"), Some("1"));
    Ok(())
}

#[test]
fn missing_or_deny_all_policy_denies_egress() {
    for policy in [None, Some(&NetworkPolicy::DenyAll)] {
        assert!(matches!(
            map_egress_route(policy),
            EgressRoute::Direct(EgressPolicy::DenyAll)
        ));
    }
}

#[test]
fn allow_all_policy_shares_network() {
    assert!(matches!(
        map_egress_route(Some(&NetworkPolicy::AllowAll)),
        EgressRoute::Direct(EgressPolicy::AllowAll)
    ));
}

#[test]
fn the_backend_none_refuses_a_policy_it_cannot_enforce() {
    for policy in [NetworkPolicy::DenyAll, user_defined()] {
        let refused = ensure_network_policy_fits_backend(IsolationBackendKind::None, Some(&policy)).err();
        assert!(matches!(
            refused
                .as_ref()
                .and_then(|e| e.downcast_ref::<trogon_sandbox_contract::LeaseError>()),
            Some(trogon_sandbox_contract::LeaseError::InvalidNetworkPolicy { .. })
        ));
    }
    for policy in [None, Some(&NetworkPolicy::AllowAll)] {
        assert!(ensure_network_policy_fits_backend(IsolationBackendKind::None, policy).is_ok());
    }
}

#[test]
fn a_confining_backend_accepts_deny_all() {
    #[cfg(target_os = "linux")]
    let confining = IsolationBackendKind::Bubblewrap;
    #[cfg(target_os = "macos")]
    let confining = IsolationBackendKind::Seatbelt;
    assert!(ensure_network_policy_fits_backend(confining, Some(&NetworkPolicy::DenyAll)).is_ok());
}

#[test]
fn user_defined_policy_goes_through_the_lease_proxy() {
    let policy = user_defined();
    assert!(matches!(
        map_egress_route(Some(&policy)),
        EgressRoute::ThroughProxy(routed) if routed == &policy
    ));
}
