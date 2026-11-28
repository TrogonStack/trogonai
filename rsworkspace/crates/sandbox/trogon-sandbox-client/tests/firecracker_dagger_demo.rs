//! Demo: a real Dagger pipeline inside one Firecracker lease, reached through the broker.
//!
//! The lease is a microVM with a network device whose policy allows only the registry the pipeline pulls from. The
//! Dagger engine runs inside the guest, so every container the pipeline starts is a container inside that VM, and its
//! egress goes through the same lease proxy as everything else.
//!
//! Prereqs: everything `firecracker_network_smoke` needs, with a worker serving a root filesystem built with
//! `build-guest-rootfs.sh --with-dagger`. Run it with `mise run sandbox:demo-dagger-firecracker`.

mod common;
mod firecracker_support;
mod stack_support;

use std::time::Instant;

use common::TestResult;
use firecracker_support::{Fallible, lease_request, sh_within, spec_parts, succeeded, with_lease};
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{
    DomainPattern, DomainRule, DomainRuleAction, LeaseId, LeaseRequest, NetworkPolicy, OpaqueTunnelPolicy,
};

const TENANT: &str = "demo-tenant";
/// The image the demo serves when `SANDBOX_FIRECRACKER_IMAGE` is unset.
const DAGGER_IMAGE: &str = "guest:dagger";
/// Docker Hub: the registry API, its token service, and the CDNs its blobs redirect to.
const REGISTRY_HOSTS: [&str; 4] = [
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
    "production.cloudfront.docker.com",
];
const PIPELINE_IMAGE: &str = "alpine:3.20";
const PROBE_IMAGE: &str = "curlimages/curl:8.10.1";
/// Run in a pipeline container: an allowed host and a denied one through the lease proxy, which the engine hands to
/// every container it starts, then the allowed host around it. No commas or double quotes: the CLI reads `--args`
/// as CSV.
const EGRESS_PROBE: &str = "echo proxy variables: $(env | grep -io ^[a-z_]*proxy | sort); \
     for u in https://auth.docker.io/token https://example.com; do \
     if curl -fsS -m 10 -o /dev/null $u; then echo $u reached; else echo $u blocked; fi; done; \
     if curl -fsS -m 10 --noproxy \\* -o /dev/null https://auth.docker.io/token; \
     then echo direct reached; else echo direct blocked; fi";
const MEMORY_MIB: u32 = 2048;
const TTL_SECONDS: u32 = 900;

fn registries_only() -> Fallible<NetworkPolicy> {
    let rules = REGISTRY_HOSTS
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

fn request() -> Fallible<LeaseRequest> {
    let mut parts = spec_parts(600, MEMORY_MIB)?;
    parts.image = stack_support::registry_image(&stack_support::env_or("SANDBOX_FIRECRACKER_IMAGE", DAGGER_IMAGE));
    parts.network_policy = Some(registries_only()?);
    lease_request(TENANT, parts, TTL_SECONDS, "dagger-demo")
}

/// Runs `script` in the guest and returns its stdout, failing unless it exited 0.
async fn step(client: &SandboxClient, lease_id: &LeaseId, script: &str, timeout_seconds: u32) -> Fallible<String> {
    succeeded(&sh_within(client, lease_id, script, timeout_seconds).await?)
}

/// The CLI only names the failed call; the reason is in the engine's log.
fn reporting_engine_log(pipeline: &str) -> String {
    format!("{pipeline} || {{ status=$?; journalctl -u dagger-engine --no-pager -n 40 >&2; exit $status; }}")
}

async fn timed<T>(label: &str, work: impl std::future::Future<Output = T>) -> T {
    let started = Instant::now();
    let result = work.await;
    println!("{label}: {:.1?}", started.elapsed());
    result
}

#[tokio::test]
#[ignore = "requires a networked firecracker worker serving a --with-dagger rootfs; run `mise run sandbox:demo-dagger-firecracker`"]
async fn a_dagger_pipeline_runs_inside_a_firecracker_lease_that_reaches_only_its_registry() -> TestResult {
    let client = &stack_support::connect().await?;
    with_lease(client, request()?, |granted| async move {
        let lease = &granted.lease_id;
        println!("{}", step(client, lease, "dagger version", 30).await?);
        timed("engine start", step(client, lease, "dagger-engine-up", 120)).await?;

        let pipeline = format!(
            "dagger core container from --address={PIPELINE_IMAGE} \
             with-new-file --path=/greeting --contents='hello from dagger in a microVM' \
             with-exec --args=sh,-c,'cat /greeting; echo; cat /etc/alpine-release' \
             stdout"
        );
        let stdout = timed("pipeline", step(client, lease, &reporting_engine_log(&pipeline), 600)).await?;
        println!("{stdout}");
        assert!(stdout.contains("hello from dagger in a microVM"), "{stdout}");
        assert!(stdout.contains("3.20."), "{stdout}");

        let probe = format!(
            "dagger core container from --address={PROBE_IMAGE} \
             with-exec --args=sh,-c,'{EGRESS_PROBE}' \
             stdout"
        );
        let verdict = timed(
            "egress from a pipeline container",
            step(client, lease, &reporting_engine_log(&probe), 300),
        )
        .await?;
        println!("{verdict}");
        assert!(verdict.contains("https://auth.docker.io/token reached"), "{verdict}");
        assert!(verdict.contains("https://example.com blocked"), "{verdict}");
        assert!(verdict.contains("direct blocked"), "{verdict}");
        Ok(())
    })
    .await
}
