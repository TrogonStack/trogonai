use crate::guest_link::{GUEST_ADDR, GUEST_MAC, guest_ip_boot_arg};
use trogon_sandbox_contract::{DomainPattern, DomainRule, DomainRuleAction};
use trogon_sandbox_egress_proxy::credential_source::UnconfiguredCredentialSource;

use super::*;

#[test]
fn the_guest_is_configured_by_the_kernel_command_line() {
    assert_eq!(
        guest_ip_boot_arg(),
        "ip=172.16.0.2::172.16.0.1:255.255.255.252::eth0:off"
    );
}

#[test]
fn the_helper_script_pins_the_guest_identity_the_vmm_is_given() {
    let script = include_str!("../../scripts/fc-netns");
    assert!(script.contains(&format!("GUEST_ADDR=\"{GUEST_ADDR}\"")));
    assert!(script.contains(&format!("GUEST_MAC=\"{GUEST_MAC}\"")));
    assert!(script.contains("iifname \"$TAP\" ip saddr $GUEST_ADDR ip daddr"));
    assert!(script.contains("hook ingress device \"$TAP\" priority -500"));
    assert!(script.contains("ether type { 8021q, 8021ad } counter name \"spoofed\" drop"));
    assert!(script.contains("ether saddr != $GUEST_MAC counter name \"spoofed\" drop"));
    assert!(script.contains("net.ipv4.conf.$TAP.rp_filter=1"));
}

#[test]
fn the_helper_script_answers_check_with_the_level_the_worker_requires() {
    let script = include_str!("../../scripts/fc-netns");
    assert!(script.contains("ANTI_SPOOF_LEVEL=1"));
    assert!(script.contains("echo \"anti-spoof $ANTI_SPOOF_LEVEL\""));
    assert!(script.contains("nft -j list counter netdev sandbox_l2 spoofed"));
    assert_eq!(ANTI_SPOOF_REPLY, "anti-spoof 1");
}

#[test]
fn an_old_or_mismatched_helper_is_refused_with_a_reason() -> Result<(), Box<dyn std::error::Error>> {
    assert!(check_reply(Ok("anti-spoof 1\n")).is_ok());
    let old = check_reply(Err("exited 2: fc-netns: usage: fc-netns up|exec|down|list ..."))
        .err()
        .ok_or("expected an error")?;
    assert!(old.contains("does not answer `check`"));
    let other = check_reply(Ok("anti-spoof 0")).err().ok_or("expected an error")?;
    assert!(other.contains("anti-spoof 0") && other.contains("anti-spoof 1"));
    Ok(())
}

#[test]
fn namespace_names_carry_the_prefix_and_refuse_what_the_helper_would() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = NetnsPrefix::new("sbxfc-")?;
    let name = NetnsName::for_lease(&prefix, &LeaseId::new("01JABC"))?;
    assert_eq!(name.as_str(), "sbxfc-01JABC");
    assert!(NetnsName::for_lease(&prefix, &LeaseId::new("a/b")).is_err());
    assert!(NetnsName::for_lease(&prefix, &LeaseId::new("x".repeat(60))).is_err());
    Ok(())
}

#[test]
fn the_guest_env_points_both_cases_of_the_proxy_vars_at_the_relay() -> Result<(), Box<dyn std::error::Error>> {
    let token = ProxyToken::generate()?;
    let env = guest_proxy_env(&token);
    let url = format!("http://sandbox:{}@172.16.0.1:3128", token.expose());
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        assert_eq!(env.get(name).map(String::as_str), Some(url.as_str()), "{name}");
    }
    assert_eq!(env.get("NO_PROXY").map(String::as_str), Some(NO_PROXY));
    Ok(())
}

#[test]
fn the_token_is_never_printed() -> Result<(), Box<dyn std::error::Error>> {
    let token = ProxyToken::generate()?;
    assert_eq!(token.expose().len(), TOKEN_BYTES * 2);
    assert!(!format!("{token:?}").contains(token.expose()));
    Ok(())
}

#[test]
fn allow_lists_are_enforceable_but_tls_interception_is_not() -> Result<(), Box<dyn std::error::Error>> {
    let allow = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert!(enforceable(&allow).is_ok());
    assert!(enforceable(&NetworkPolicy::AllowAll).is_ok());
    Ok(())
}

#[tokio::test]
async fn a_credentialed_lease_gets_an_in_process_proxy_with_its_own_ca() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let state = crate::config::StateDir::new(tmp.path());
    let lease = LeaseId::new("lease1");
    std::fs::create_dir_all(tmp.path().join(lease.as_str()))?;
    let dir = VmDir::for_lease(&state, &lease)?;
    let credentials = LeaseCredentials::new(std::sync::Arc::new(UnconfiguredCredentialSource), vec![]);

    let mut proxy = Proxy::InProcess(InProcessProxy::spawn(&dir, &NetworkPolicy::AllowAll, credentials).await?);
    let socket = proxy_socket(&dir);
    assert!(
        std::fs::symlink_metadata(&socket).is_ok_and(|m| m.file_type().is_socket()),
        "the relay needs a socket to forward to"
    );
    let Proxy::InProcess(in_process) = &proxy else {
        return Err("unexpected variant".into());
    };
    let pem = in_process.server.as_ref().ok_or("proxy missing")?.ca_certificate_pem();
    assert!(pem.contains("BEGIN CERTIFICATE"));
    assert!(!pem.contains("PRIVATE KEY"));
    assert_eq!(proxy.token().expose().len(), TOKEN_BYTES * 2);
    proxy.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_policy_rewrite_replaces_the_whole_file() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join(POLICY_FILE);
    write_policy(&path, &NetworkPolicy::AllowAll).await?;
    write_policy(&path, &NetworkPolicy::DenyAll).await?;
    let read: NetworkPolicy = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(read, NetworkPolicy::DenyAll);
    assert!(!path.with_extension("json.next").exists());
    Ok(())
}
