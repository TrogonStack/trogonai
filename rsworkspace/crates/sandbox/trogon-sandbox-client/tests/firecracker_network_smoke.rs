//! End-to-end: a Firecracker lease with a network device, reached the way callers reach it, through the broker. The
//! guest's only way out is its lease's egress proxy, so what it can reach is exactly what its `NetworkPolicy` allows,
//! and a policy update changes that live.
//!
//! Prereqs: everything `firecracker_stack_smoke` needs, with a worker that sets up lease networking, and a host that
//! can reach `ALLOWED_HOST` and `OTHER_HOST` over HTTPS. See `mise run sandbox:smoke-firecracker`
//! (`SANDBOX_SMOKE_FIRECRACKER_NETWORK=1`).

mod common;
mod firecracker_support;
mod stack_support;

use std::time::{Duration, Instant};

use common::TestResult;
use firecracker_support::{Fallible, guest_sh, lease_request, spec_parts, succeeded, with_lease};
use trogon_sandbox_client::{ClientError, SandboxClient};
use trogon_sandbox_contract::{
    DomainPattern, DomainRule, DomainRuleAction, LeaseId, LeaseRequest, NetworkPolicy, OpaqueTunnelPolicy,
};

const TENANT: &str = "smoke-tenant";
const ALLOWED_HOST: &str = "example.com";
const OTHER_HOST: &str = "example.org";
/// Where a guest would go if it ignored its proxy: a public resolver, and the proxy's own host on a port that is not
/// the proxy.
const DIRECT_TARGETS: [&str; 2] = ["https://1.1.1.1", "http://172.16.0.1:22"];
const APPLY_BUDGET: Duration = Duration::from_secs(15);

fn request(network_policy: Option<NetworkPolicy>) -> Fallible<LeaseRequest> {
    let mut parts = spec_parts(30, 256)?;
    parts.network_policy = network_policy;
    lease_request(TENANT, parts, 180, "net-smoke")
}

fn allow(hosts: &[&str]) -> Fallible<NetworkPolicy> {
    let rules = hosts
        .iter()
        .map(|host| {
            Ok(DomainRule {
                domain: DomainPattern::new(host)?,
                action: DomainRuleAction::PassThrough,
            })
        })
        .collect::<Fallible<Vec<_>>>()?;
    Ok(NetworkPolicy::UserDefined {
        rules,
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: OpaqueTunnelPolicy::default(),
        request_rules: vec![],
    })
}

async fn interfaces(client: &SandboxClient, lease_id: &LeaseId) -> Fallible<Vec<String>> {
    let listing = succeeded(&guest_sh(client, lease_id, "ls /sys/class/net").await?)?;
    Ok(listing.split_whitespace().map(String::from).collect())
}

/// Whether an HTTPS GET through the lease's proxy got any answer from `host`.
async fn reaches(client: &SandboxClient, lease_id: &LeaseId, host: &str) -> Fallible<bool> {
    let output = guest_sh(
        client,
        lease_id,
        &format!("curl -sS -m 10 -o /dev/null -w '%{{http_code}}' https://{host}/"),
    )
    .await?;
    Ok(output.exit_code == 0 && stack_support::stdout(&output).trim().starts_with(['2', '3']))
}

async fn eventually_reaches(client: &SandboxClient, lease_id: &LeaseId, host: &str, want: bool) -> Fallible<()> {
    let deadline = Instant::now() + APPLY_BUDGET;
    while reaches(client, lease_id, host).await? != want {
        if Instant::now() >= deadline {
            return Err(format!("{host} reachable={} after {APPLY_BUDGET:?}, wanted {want}", !want).into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok(())
}

/// Bypassing the proxy must fail even though the guest has a working interface.
async fn assert_no_direct_egress(client: &SandboxClient, lease_id: &LeaseId) -> Fallible<()> {
    for target in DIRECT_TARGETS {
        let output = guest_sh(
            client,
            lease_id,
            &format!("curl -sS -m 5 --noproxy '*' -o /dev/null {target}"),
        )
        .await?;
        assert_ne!(
            output.exit_code, 0,
            "{target} was reachable without the proxy: {output:?}"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a networked firecracker worker; run `mise run sandbox:smoke-firecracker` with SANDBOX_SMOKE_FIRECRACKER_NETWORK=1"]
async fn a_lease_without_an_egress_policy_still_boots_with_no_network_device() -> TestResult {
    let client = &stack_support::connect().await?;
    for policy in [None, Some(NetworkPolicy::DenyAll)] {
        with_lease(client, request(policy)?, |granted| async move {
            let lease = &granted.lease_id;
            assert_eq!(interfaces(client, lease).await?, ["lo"]);
            assert_eq!(
                succeeded(&guest_sh(client, lease, "echo \"$HTTPS_PROXY\"").await?)?.trim(),
                ""
            );

            // Keeping a closed lease closed is a no-op.
            client.update_network_policy(lease, NetworkPolicy::DenyAll).await?;
            let opened = client.update_network_policy(lease, allow(&[ALLOWED_HOST])?).await;
            assert!(
                matches!(opened, Err(ClientError::Denied(_))),
                "opening a lease with no network device was not denied: {opened:?}"
            );
            Ok(())
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a networked firecracker worker; run `mise run sandbox:smoke-firecracker` with SANDBOX_SMOKE_FIRECRACKER_NETWORK=1"]
async fn an_allow_listed_destination_is_reachable_and_nothing_else_is() -> TestResult {
    let client = &stack_support::connect().await?;
    let req = request(Some(allow(&[ALLOWED_HOST])?))?;
    with_lease(client, req, |granted| async move {
        let lease = &granted.lease_id;
        assert_eq!(interfaces(client, lease).await?, ["eth0", "lo"]);
        assert!(reaches(client, lease, ALLOWED_HOST).await?);
        assert!(!reaches(client, lease, OTHER_HOST).await?);
        assert_no_direct_egress(client, lease).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires a networked firecracker worker; run `mise run sandbox:smoke-firecracker` with SANDBOX_SMOKE_FIRECRACKER_NETWORK=1"]
async fn a_policy_update_applies_to_the_running_lease() -> TestResult {
    let client = &stack_support::connect().await?;
    let req = request(Some(allow(&[ALLOWED_HOST])?))?;
    with_lease(client, req, |granted| async move {
        let lease = &granted.lease_id;
        assert!(!reaches(client, lease, OTHER_HOST).await?);

        // Widening the allow-list is applied.
        client
            .update_network_policy(lease, allow(&[ALLOWED_HOST, OTHER_HOST])?)
            .await?;
        eventually_reaches(client, lease, OTHER_HOST, true).await?;
        assert!(reaches(client, lease, ALLOWED_HOST).await?);

        // Closing a networked lease is applied.
        client.update_network_policy(lease, NetworkPolicy::DenyAll).await?;
        eventually_reaches(client, lease, ALLOWED_HOST, false).await?;
        assert!(!reaches(client, lease, OTHER_HOST).await?);
        assert_no_direct_egress(client, lease).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires a networked firecracker worker; run `mise run sandbox:smoke-firecracker` with SANDBOX_SMOKE_FIRECRACKER_NETWORK=1"]
async fn allow_all_still_goes_through_the_proxy() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request(Some(NetworkPolicy::AllowAll))?, |granted| async move {
        let lease = &granted.lease_id;
        assert!(reaches(client, lease, ALLOWED_HOST).await?);
        assert!(reaches(client, lease, OTHER_HOST).await?);
        assert_no_direct_egress(client, lease).await
    })
    .await
}
