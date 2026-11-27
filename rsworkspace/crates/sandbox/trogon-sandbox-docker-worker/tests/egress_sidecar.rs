//! Live check of the egress sidecar hardening against a real Docker daemon: the sidecar's capability set, pids and
//! swap limits, process privileges, and the proxy's allow and deny behaviour for the workload.
//!
//! Needs the `sandbox-egress-proxyd:dev` image (built from the `trogon-sandbox-egress-proxy` crate; override with
//! `SANDBOX_EGRESS_PROXY_IMAGE`) and the cached `busybox:1.36.1` stub image.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker --test egress_sidecar -- --ignored --nocapture
//! ```

mod docker_support;

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use docker_support::{ALPINE_IMAGE, TestResult, fresh_id, fresh_lease_id};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    BootMode, CommandEnv, DeniedDomain, DomainPattern, DomainRule, DomainRuleAction, IpRange, LeaseId, LeaseRequest,
    LeaseRequestKey, NetworkPolicy, OciImageRef, OpaqueTunnelPolicy, ProviderKind, ProxyUid, SandboxImage, SandboxSpec,
    SandboxSpecParts, SandboxTags, StreamedOutput, TenantId, WorkspaceSource,
};
use trogon_sandbox_docker_worker::adapter::{self, DockerWorkerProvider};
use trogon_sandbox_docker_worker::digest_cache::DigestCache;
use trogon_sandbox_protocol::Provider as _;
use trogon_sandbox_providers::{ContainerHardening, LocalDockerProvider};
use trogon_sandbox_volumes::VolumeRegion;

const STUB_IMAGE: &str = "busybox:1.36.1";
const DEFAULT_EGRESS_IMAGE: &str = "sandbox-egress-proxyd:dev";
const ALLOWED_PORT: u16 = 18081;
const DENIED_PORT: u16 = 18082;

/// `SANDBOX_EGRESS_PROXY_IMAGE` overrides the sidecar image under test.
fn egress_image() -> String {
    trogon_std::env::ReadEnv::var(&trogon_std::env::SystemEnv, "SANDBOX_EGRESS_PROXY_IMAGE")
        .unwrap_or_else(|_| DEFAULT_EGRESS_IMAGE.to_string())
}

/// Runs the Docker CLI and returns its trimmed stdout, or `None` when it fails.
fn docker(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("docker")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn image_present(image: &str) -> bool {
    docker(&["image", "inspect", "--format", "{{.Id}}", image]).is_some()
}

/// Removes the stub server container, by name, when the test ends.
struct StubServer {
    name: String,
    ip: String,
}

impl StubServer {
    /// Serves a fixed body on both test ports, so a refused connection can only come from the egress proxy, never
    /// from a closed port.
    fn start(prefix: &str) -> TestResult<Self> {
        let name = format!("{prefix}-{}", fresh_id().to_ascii_lowercase());
        docker(&[
            "run",
            "-d",
            "--name",
            &name,
            STUB_IMAGE,
            "sh",
            "-c",
            &format!(
                "mkdir /www && echo stub-ok > /www/index.html && \
                 httpd -p {ALLOWED_PORT} -h /www && httpd -f -p {DENIED_PORT} -h /www"
            ),
        ])
        .ok_or("start the stub server")?;
        let mut stub = Self {
            name,
            ip: String::new(),
        };
        stub.ip = docker(&[
            "inspect",
            "--format",
            "{{.NetworkSettings.Networks.bridge.IPAddress}}",
            &stub.name,
        ])
        .ok_or("inspect the stub server address")?;
        Ok(stub)
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.name]);
    }
}

fn lease_request(stub_ip: &str) -> TestResult<LeaseRequest> {
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new(&format!("{stub_ip}:{ALLOWED_PORT}"))?,
            action: DomainRuleAction::PassThrough,
        }],
        // The proxy refuses private destinations unless an allow range names them. The range admits every port on
        // the stub, so the second port is denied by name.
        allow_ranges: vec![IpRange::new(format!("{stub_ip}/32"))?],
        deny_ranges: vec![],
        denied_domains: vec![DeniedDomain::new(&format!("{stub_ip}:{DENIED_PORT}"))?],
        opaque_tunnels: OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(""),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: std::num::NonZeroU32::MIN,
        memory_mib: std::num::NonZeroU32::new(256).ok_or("memory")?,
        snapshot_out: None,
        network_policy: Some(policy),
        mounts: vec![],
        drives: vec![],
        exposed_ports: vec![],
        boot_mode: BootMode::Bare,
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("egress-sidecar-test")?,
        provider_pref: vec![ProviderKind::LocalDocker],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new(fresh_id())?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: Default::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: Default::default(),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

async fn build_provider(server: &JetStreamTestServer) -> TestResult<Arc<DockerWorkerProvider>> {
    let inner = LocalDockerProvider::with_default_engine(egress_image())?
        .with_hardening(ContainerHardening::secure())
        .with_owner(format!("egress-sidecar-test-{}", fresh_id()));
    Ok(Arc::new(DockerWorkerProvider::new(
        inner,
        ALPINE_IMAGE.to_string(),
        server.jetstream().await,
        server.address().to_string(),
        "egress-sidecar-test-worker".to_string(),
        adapter::build_oci_client(vec![]),
        "docker.io".to_string(),
        None,
        Arc::new(DigestCache::new(8)),
        VolumeRegion::new("us-east-1").ok(),
        None,
        adapter::default_warm_pool_config(),
    )?))
}

async fn sh(provider: &DockerWorkerProvider, lease: &LeaseId, script: &str) -> TestResult<(i32, String)> {
    let out = provider
        .run_command_for_lease(
            lease,
            vec!["sh".into(), "-c".into(), script.into()],
            None,
            &CommandEnv::default(),
            30,
            None,
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    Ok((
        out.exit_code,
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    ))
}

fn inspect(container: &str, format: &str) -> TestResult<String> {
    Ok(docker(&["inspect", "--format", format, container]).ok_or("docker inspect")?)
}

/// The sidecar shares the workload's network namespace, so it is the container whose `NetworkMode` names the
/// workload.
fn find_sidecar(workload_id: &str) -> TestResult<String> {
    let candidates = docker(&["ps", "-q", "--filter", "name=-egress-proxy"]).unwrap_or_default();
    for id in candidates.lines() {
        if inspect(id, "{{.HostConfig.NetworkMode}}")?.contains(workload_id) {
            return Ok(id.to_string());
        }
    }
    Err("no egress sidecar shares the workload's network".into())
}

fn proc_status_field(status: &str, key: &str) -> TestResult<String> {
    Ok(status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .ok_or_else(|| format!("{key} missing from /proc status"))?
        .trim()
        .to_string())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a Docker daemon, the sandbox-egress-proxyd:dev image and the cached busybox:1.36.1 image; run with --ignored"]
async fn the_egress_sidecar_is_hardened_and_enforces_the_policy() -> TestResult {
    let image = egress_image();
    if !image_present(&image) {
        return Err(format!("build {image} from the trogon-sandbox-egress-proxy crate first").into());
    }
    let server = JetStreamTestServer::start().await;
    let provider = build_provider(&server).await?;
    let stub = StubServer::start("egress-stub")?;
    // A second host serving the same port, which the policy never mentions.
    let unlisted = StubServer::start("egress-unlisted")?;

    let lease_id = fresh_lease_id();
    provider.lease(&lease_request(&stub.ip)?, &lease_id).await?;

    let result = assertions(&provider, &lease_id, &stub.ip, &unlisted.ip).await;
    provider.cancel(&lease_id).await?;
    let sidecar = result?;
    // Killing the workload must also stop a sidecar that shares its network.
    let running = docker(&["inspect", "--format", "{{.State.Running}}", &sidecar]);
    if running.as_deref() == Some("true") {
        let _ = docker(&["rm", "-f", &sidecar]);
        return Err("the egress sidecar outlived the lease".into());
    }
    Ok(())
}

/// Returns the sidecar's container id so the caller can check teardown.
async fn assertions(
    provider: &DockerWorkerProvider,
    lease: &LeaseId,
    stub_ip: &str,
    unlisted_ip: &str,
) -> TestResult<String> {
    // The sidecar installs its redirect after the workload starts; wait for it.
    let fetch = |port: u16| format!("wget -q -T 5 -O - http://{stub_ip}:{port}/");
    let mut allowed = (1, String::new());
    for _ in 0..30 {
        allowed = sh(provider, lease, &fetch(ALLOWED_PORT)).await?;
        if allowed.0 == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if allowed.0 != 0 {
        let (_, workload_id) = sh(provider, lease, "hostname").await?;
        let logs = docker(&["logs", &find_sidecar(workload_id.trim())?]).unwrap_or_default();
        return Err(format!("allowed host must be reachable: {}\nsidecar log:\n{logs}", allowed.1).into());
    }
    assert!(allowed.1.contains("stub-ok"), "unexpected body: {}", allowed.1);

    let denied = sh(provider, lease, &fetch(DENIED_PORT)).await?;
    assert_ne!(denied.0, 0, "a denied destination must be refused: {}", denied.1);
    assert!(!denied.1.contains("stub-ok"), "denied request got a body");

    // A host the policy never lists is refused, whatever port it serves.
    let unlisted = sh(
        provider,
        lease,
        &format!("wget -q -T 5 -O - http://{unlisted_ip}:{ALLOWED_PORT}/"),
    )
    .await?;
    assert_ne!(
        unlisted.0, 0,
        "a destination the policy never lists must be refused: {}",
        unlisted.1
    );
    assert!(!unlisted.1.contains("stub-ok"), "unlisted request got a body");

    // The workload cannot change uid (the proxy is exempted by uid), so SETUID and SETGID are gone from its
    // bounding set (bits 6 and 7).
    let (_, workload_id) = sh(provider, lease, "hostname").await?;
    let workload_id = workload_id.trim();
    let (_, status) = sh(provider, lease, "cat /proc/self/status").await?;
    let bounding = u64::from_str_radix(&proc_status_field(&status, "CapBnd:")?, 16)?;
    assert_eq!(bounding & (1 << 6 | 1 << 7), 0, "workload kept SETUID/SETGID");

    let sidecar = find_sidecar(workload_id)?;

    // Container-level hardening, as Docker recorded it.
    assert_eq!(inspect(&sidecar, "{{json .HostConfig.CapDrop}}")?, r#"["ALL"]"#);
    let mut added: Vec<String> = serde_json::from_str(&inspect(&sidecar, "{{json .HostConfig.CapAdd}}")?)?;
    for capability in &mut added {
        *capability = capability.trim_start_matches("CAP_").to_string();
    }
    added.sort();
    assert_eq!(added, ["NET_ADMIN", "SETGID", "SETPCAP", "SETUID"]);
    assert_eq!(inspect(&sidecar, "{{.HostConfig.PidsLimit}}")?, "256");
    let memory = inspect(&sidecar, "{{.HostConfig.Memory}}")?;
    assert_eq!(memory, (128 * 1024 * 1024).to_string());
    assert_eq!(
        inspect(&sidecar, "{{.HostConfig.MemorySwap}}")?,
        memory,
        "swap must be pinned to the memory limit"
    );
    assert_eq!(
        inspect(&sidecar, "{{.HostConfig.ReadonlyRootfs}}")?,
        "true",
        "the sidecar rootfs must be read-only"
    );
    assert_eq!(
        inspect(&sidecar, "{{json .HostConfig.SecurityOpt}}")?,
        r#"["no-new-privileges"]"#
    );

    // The proxy process itself: unprivileged uid, empty bounding set, no new privileges.
    let proc = docker(&["exec", &sidecar, "cat", "/proc/1/status"]).ok_or("read proxy status")?;
    assert_eq!(
        proc_status_field(&proc, "Uid:")?.split_whitespace().next(),
        Some(ProxyUid::EGRESS_PROXY.to_string().as_str())
    );
    assert_eq!(
        proc_status_field(&proc, "CapBnd:")?,
        "0000000000000000",
        "proxy kept bounding capabilities"
    );
    assert_eq!(proc_status_field(&proc, "NoNewPrivs:")?, "1");

    // Even as root in the sidecar, the policy file is a read-only mount.
    assert!(
        docker(&[
            "exec",
            "-u",
            "0",
            &sidecar,
            "sh",
            "-c",
            "echo x >> /etc/sandbox-egress-proxy/policy.json",
        ])
        .is_none(),
        "the policy file must not be writable from the sidecar"
    );

    Ok(sidecar)
}
