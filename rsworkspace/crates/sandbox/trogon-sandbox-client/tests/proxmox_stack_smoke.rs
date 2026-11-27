//! Live lane for `trogon-sandbox-proxmox-worker` behind a broker. Run it against an isolated broker and NATS with a
//! real Proxmox worker, using `--ignored --nocapture --test-threads=1`. See `stack_support` for how the lane finds
//! NATS. The owner of the fixture provisions the template, the bridge and the live target services.
//!
//! Settings:
//!
//! - `SANDBOX_SMOKE_PROXMOX_IMAGE`: the template image the worker clones (required).
//! - `SANDBOX_SMOKE_TENANT`: the tenant the leases run under (default `pve-live-test`).
//! - `SANDBOX_SMOKE_API_KEY`: the broker API key, when the broker requires one.
//! - `SANDBOX_SMOKE_PROXMOX_DIAGNOSTICS`: `1` prints guest boot diagnostics as JSON lines.
//!
//! The proxy test also needs `SANDBOX_SMOKE_PROXMOX_ALLOWED_DOMAIN`, `SANDBOX_SMOKE_PROXMOX_DENIED_DOMAIN`,
//! `SANDBOX_SMOKE_PROXMOX_ALLOWED_URL` and `SANDBOX_SMOKE_PROXMOX_DENIED_URL` (two distinct `http://` URLs, one
//! served by the allowed domain and one by the denied one), `SANDBOX_SMOKE_PROXMOX_DIRECT_TARGETS` (a comma
//! separated list of at most eight known-listening `ip:port` addresses the guest must not reach directly), and
//! optionally `SANDBOX_SMOKE_PROXMOX_ALLOW_IP`, an address added to the policy as a single-address range.

mod common;
mod stack_support;

use std::future::Future;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use async_nats::Subscriber;
use common::TestResult;
use futures::StreamExt;
use trogon_sandbox_client::{CommandOptions, SandboxClient};
use trogon_sandbox_contract::{
    AllowRange, CommandOutput, DeniedDomain, DomainPattern, DomainRule, DomainRuleAction, IpRange, LeaseGranted,
    LeaseId, LeasePersistence, LeaseReleased, LeaseReleasedReason, LeaseRequest, NetworkPolicy, OpaqueTunnelPolicy,
    ProviderKind, SandboxFileWrite, SandboxSpec, TenantId,
};

const DEFAULT_TENANT: &str = "pve-live-test";
const RELEASED_FILTER: &str = "sandbox.lease.released.>";
const MAX_DIRECT_TARGETS: usize = 8;

/// Fails with `message` unless `condition` holds.
fn ensure(condition: bool, message: impl Into<String>) -> TestResult {
    if condition { Ok(()) } else { Err(message.into().into()) }
}

fn request(policy: NetworkPolicy) -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let image = stack_support::required_env("SANDBOX_SMOKE_PROXMOX_IMAGE")?;
    let mut parts = stack_support::spec_parts(stack_support::registry_image(&image), 60, 512)?;
    parts.network_policy = Some(policy);
    let tenant = TenantId::new(stack_support::env_or("SANDBOX_SMOKE_TENANT", DEFAULT_TENANT))?;
    let mut request =
        stack_support::lease_request(tenant, ProviderKind::Proxmox, SandboxSpec::new(parts)?, 300, "pve-live")?;
    request.persistent = LeasePersistence::Ephemeral;
    Ok(request)
}

fn client_for(nats: &async_nats::Client) -> SandboxClient {
    let client = SandboxClient::from_client(nats.clone());
    match stack_support::var("SANDBOX_SMOKE_API_KEY") {
        Ok(key) => client.with_api_key(key),
        Err(_) => client,
    }
}

/// Waits for the broker to announce that `lease_id` was released, and checks the announcement.
async fn confirmed_release(
    mut released: Subscriber,
    lease_id: &LeaseId,
    tenant: &TenantId,
) -> Result<LeaseReleased, Box<dyn std::error::Error>> {
    while let Some(message) = released.next().await {
        let receipt: LeaseReleased = serde_json::from_slice(&message.payload)?;
        if &receipt.lease_id == lease_id {
            ensure(
                &receipt.tenant == tenant && receipt.reason == LeaseReleasedReason::Cancelled,
                "broker release identity or reason differed",
            )?;
            ensure(
                receipt.worker_id.is_some(),
                "release omitted the stable worker identity",
            )?;
            return Ok(receipt);
        }
    }
    Err("release subscription closed".into())
}

/// Leases `request`, runs `body` against the grant, then cancels the lease and waits for the broker to confirm the
/// worker released it, whether or not `body` succeeded.
async fn with_lease<F, Fut>(request: LeaseRequest, body: F) -> TestResult
where
    F: FnOnce(SandboxClient, LeaseGranted) -> Fut,
    Fut: Future<Output = TestResult>,
{
    let nats = tokio::time::timeout(Duration::from_secs(10), async_nats::connect(stack_support::nats_url())).await??;
    let released = nats.subscribe(RELEASED_FILTER).await?;
    nats.flush().await?;
    let client = client_for(&nats);
    let cleanup_client = client_for(&nats);
    let granted = client.lease(&request, Duration::from_secs(180)).await?;
    let lease_id = granted.lease_id.clone();
    println!(
        "{}",
        serde_json::json!({"phase":"granted","lease_id":lease_id,"vm_id":granted.sandbox_id})
    );
    let outcome = tokio::time::timeout(Duration::from_secs(180), body(client, granted)).await;
    let cancelled = cleanup_client.cancel(&lease_id).await;
    nats.flush().await?;
    cancelled?;
    let receipt = tokio::time::timeout(
        Duration::from_secs(150),
        confirmed_release(released, &lease_id, &request.tenant),
    )
    .await
    .map_err(|_| "broker never confirmed exact worker cleanup")??;
    println!(
        "{}",
        serde_json::json!({"phase":"released","lease_id":receipt.lease_id,"worker_id":receipt.worker_id})
    );
    outcome.map_err(|_| "live guest body exceeded 180 seconds")?
}

async fn command(
    client: &SandboxClient,
    lease: &LeaseId,
    argv: Vec<String>,
) -> Result<CommandOutput, Box<dyn std::error::Error>> {
    let options = CommandOptions {
        timeout_seconds: 15,
        ..CommandOptions::default()
    };
    Ok(client.run_command(lease, argv, options).await?)
}

/// Prints the guest's boot state as JSON lines. A command that fails is reported, not propagated.
async fn print_diagnostics(client: &SandboxClient, lease: &LeaseId) {
    for (name, script) in [
        (
            "qga-unit-and-critical-chain",
            "systemctl cat qemu-guest-agent.service; for p in /usr/lib/udev/rules.d/60-qemu-guest-agent.rules /lib/udev/rules.d/60-qemu-guest-agent.rules; do test ! -f \"$p\" || cat \"$p\"; done; systemd-analyze critical-chain qemu-guest-agent.service",
        ),
        (
            "boot-unit-timestamps",
            "systemctl show qemu-guest-agent.service systemd-networkd-wait-online.service cloud-init-local.service cloud-init.service cloud-config.service cloud-final.service sysinit.target network-online.target -p Id -p ActiveState -p Result -p ExecMainStartTimestampMonotonic -p ActiveEnterTimestampMonotonic -p After -p Wants; cloud-init status --format json; networkctl list --no-pager",
        ),
        (
            "boot-journal",
            "journalctl -b --no-pager -o short-monotonic -n 150 -u qemu-guest-agent.service -u systemd-networkd-wait-online.service -u cloud-init-local.service -u cloud-init.service -u cloud-config.service -u cloud-final.service",
        ),
    ] {
        let receipt = match command(client, lease, stack_support::sh(script)).await {
            Ok(output) => serde_json::json!({
                "phase": "diagnostic",
                "name": name,
                "lease_id": lease,
                "exit_code": output.exit_code,
                "stdout": stack_support::stdout(&output),
                "stderr": stack_support::stderr(&output),
            }),
            Err(_) => {
                serde_json::json!({"phase": "diagnostic", "name": name, "lease_id": lease, "transport_error": true})
            }
        };
        println!("{receipt}");
    }
}

async fn files_and_exec(client: &SandboxClient, granted: &LeaseGranted, networkless: bool) -> TestResult {
    ensure(
        granted.provider == ProviderKind::Proxmox,
        "broker selected a different provider",
    )?;
    let script = if networkless {
        "set -eu; test \"$(id -u)\" = 0; test \"$(ls /sys/class/net)\" = lo; printf 'guest-out\\n'; printf 'guest-err\\n' >&2; exit 7"
    } else {
        "set -eu; test \"$(id -u)\" = 0; test \"$(ls /sys/class/net | wc -l)\" -gt 1; python3 -c 'import socket'; printf 'guest-out\\n'; printf 'guest-err\\n' >&2; exit 7"
    };
    let output = command(client, &granted.lease_id, stack_support::sh(script)).await?;
    ensure(
        output.exit_code == 7 && output.stdout == b"guest-out\n" && output.stderr == b"guest-err\n",
        "guest identity, NIC or output semantics differed",
    )?;
    if stack_support::var("SANDBOX_SMOKE_PROXMOX_DIAGNOSTICS").as_deref() == Ok("1") {
        print_diagnostics(client, &granted.lease_id).await;
    }
    let content: Vec<u8> = (0..=u8::MAX).cycle().take(512 * 1024).collect();
    let path = "/workspace/broker binary; 'file'".to_string();
    client
        .write_files(
            &granted.lease_id,
            vec![SandboxFileWrite {
                path: path.clone(),
                content: content.clone(),
                mode: Some(0o600),
            }],
            None,
        )
        .await?;
    ensure(
        client.read_file(&granted.lease_id, path, None).await? == content,
        "binary file roundtrip differed",
    )
}

#[tokio::test]
#[ignore = "requires an isolated broker, NATS and real Proxmox template; creates and destroys a live clone"]
async fn broker_networkless_clone_exec_binary_files_and_confirmed_release() -> TestResult {
    with_lease(request(NetworkPolicy::DenyAll)?, |client, granted| async move {
        files_and_exec(&client, &granted, true).await
    })
    .await
}

/// The single-address range for `SANDBOX_SMOKE_PROXMOX_ALLOW_IP`, when it is set.
fn allow_ranges() -> Result<Vec<IpRange>, Box<dyn std::error::Error>> {
    match stack_support::var("SANDBOX_SMOKE_PROXMOX_ALLOW_IP") {
        Ok(address) => {
            let address: IpAddr = address.parse()?;
            let prefix = if address.is_ipv4() { 32 } else { 128 };
            Ok(vec![IpRange::new(format!("{address}/{prefix}"))?])
        }
        Err(_) => Ok(Vec::new()),
    }
}

/// The direct TCP targets, each checked to accept a connection from the harness host.
async fn direct_targets() -> Result<Vec<SocketAddr>, Box<dyn std::error::Error>> {
    let targets = stack_support::required_env("SANDBOX_SMOKE_PROXMOX_DIRECT_TARGETS")?
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<SocketAddr>, _>>()?;
    ensure(
        !targets.is_empty() && targets.len() <= MAX_DIRECT_TARGETS,
        "supply bounded known-listening direct TCP targets",
    )?;
    for target in &targets {
        let target = *target;
        tokio::task::spawn_blocking(move || TcpStream::connect_timeout(&target, Duration::from_secs(5)))
            .await?
            .map_err(|error| {
                format!("direct TCP positive control {target} is not reachable from harness host: {error}")
            })?;
    }
    Ok(targets)
}

/// Fetches `url` from the guest through the proxy and checks the proxy's verdict.
async fn proxied_status(client: &SandboxClient, lease: &LeaseId, url: String, permitted: bool) -> TestResult {
    let argv = [
        "curl",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "3",
        "--max-time",
        "8",
        "--output",
        "/dev/null",
        "--write-out",
        "%{http_code}",
    ]
    .into_iter()
    .map(String::from)
    .chain([url])
    .collect();
    let output = command(client, lease, argv).await?;
    let status = String::from_utf8(output.stdout)?.parse::<u16>()?;
    ensure(
        output.exit_code == 0,
        "proxy request failed before an HTTP policy result",
    )?;
    ensure(
        if permitted {
            (200..400).contains(&status)
        } else {
            status == 403
        },
        format!("proxy policy returned unexpected status {status}"),
    )
}

/// Checks that the guest cannot open a TCP connection to `target` without going through the proxy.
async fn direct_tcp_is_blocked(client: &SandboxClient, lease: &LeaseId, target: SocketAddr) -> TestResult {
    let probe = "import socket,sys\ns=socket.socket(socket.AF_INET6 if ':' in sys.argv[1] else socket.AF_INET)\ns.settimeout(3)\ntry:\n s.connect((sys.argv[1],int(sys.argv[2])))\nexcept OSError:\n sys.exit(73)\nfinally:\n s.close()\nsys.exit(0)";
    let argv = vec![
        "python3".to_string(),
        "-c".to_string(),
        probe.to_string(),
        target.ip().to_string(),
        target.port().to_string(),
    ];
    let output = command(client, lease, argv).await?;
    ensure(
        output.exit_code == 73,
        format!(
            "guest direct TCP target {target} was not blocked (exit {})",
            output.exit_code
        ),
    )
}

#[tokio::test]
#[ignore = "requires a real proxy-networked Proxmox worker and explicit reachable HTTP/TCP controls"]
async fn broker_proxy_allows_one_destination_denies_another_and_blocks_direct_tcp() -> TestResult {
    let domain = DomainPattern::new(&stack_support::required_env("SANDBOX_SMOKE_PROXMOX_ALLOWED_DOMAIN")?)?;
    let denied_domain = DeniedDomain::new(&stack_support::required_env("SANDBOX_SMOKE_PROXMOX_DENIED_DOMAIN")?)?;
    let allow_ranges = allow_ranges()?;
    let allowed = stack_support::required_env("SANDBOX_SMOKE_PROXMOX_ALLOWED_URL")?;
    let denied = stack_support::required_env("SANDBOX_SMOKE_PROXMOX_DENIED_URL")?;
    ensure(
        allowed.starts_with("http://") && denied.starts_with("http://") && allowed != denied,
        "HTTP control URLs must be distinct",
    )?;
    let targets = direct_targets().await?;
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: allow_ranges.into_iter().map(AllowRange::from).collect(),
        deny_ranges: Vec::new(),
        denied_domains: vec![denied_domain],
        opaque_tunnels: OpaqueTunnelPolicy::default(),
        request_rules: Vec::new(),
    };
    with_lease(request(policy)?, |client, granted| async move {
        files_and_exec(&client, &granted, false).await?;
        for (url, permitted) in [(allowed, true), (denied, false)] {
            proxied_status(&client, &granted.lease_id, url, permitted).await?;
        }
        for target in targets {
            direct_tcp_is_blocked(&client, &granted.lease_id, target).await?;
        }
        Ok(())
    })
    .await
}
