//! Runs the `worker`, `lease`, `capability`, `snapshot delete` and `substrate prebuild` commands
//! against a private NATS server, a fake registry and, for the live case, a real control plane.

use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_admin::snapshot::SnapshotDeleteRequest;
use trogon_sandbox_admin::{
    AdminError, Console, PrebuildRequest, capability, constants, lease, snapshot, substrate, worker,
};
use trogon_sandbox_broker::active_lease::{ActiveLeaseStore, NatsKvActiveLeaseStore};
use trogon_sandbox_broker::core::FREEZE_SETTLE_MAX_ATTEMPTS;
use trogon_sandbox_broker::default_capability_profile::default_capability_profile;
use trogon_sandbox_broker::freeze_admin::FreezeResolution;
use trogon_sandbox_broker::idempotency::NatsKvIdempotencyStore;
use trogon_sandbox_broker::worker_fence::WorkerPlacementState;
use trogon_sandbox_contract::{
    ArtifactType, CapabilitySupport, Digest, FreezeAction, FreezeInFlight, LeaseId, LeaseRecord, LeaseStatus,
    OciImageRef, ProviderCapabilityProfile, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags,
    TenantId, WorkerCapabilities, WorkerCapacity, WorkerHeartbeat, WorkerId, WorkspaceSource,
};
use trogon_sandbox_oci::RegistryHost;
use trogon_sandbox_protocol::subjects::worker_heartbeat;
use trogon_std::env::{ReadEnv, SystemEnv};

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct Buffers {
    out: Vec<u8>,
    err: Vec<u8>,
}

impl Buffers {
    fn out(&self) -> String {
        String::from_utf8_lossy(&self.out).into_owned()
    }

    fn err(&self) -> String {
        String::from_utf8_lossy(&self.err).into_owned()
    }
}

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("acme")?)
}

fn digest() -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::new(format!("sha256:{}", "ab".repeat(32)))?)
}

fn spec() -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(60).ok_or("zero cpu")?,
        memory_mib: NonZeroU32::new(512).ok_or("zero memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
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
    })?)
}

fn lease_on(
    id: &str,
    provider: ProviderKind,
    worker: Option<&WorkerId>,
    freeze: Option<FreezeInFlight>,
) -> Result<LeaseRecord, Box<dyn Error>> {
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: LeaseId::new(id),
        tenant: tenant()?,
        provider,
        status: LeaseStatus::Running,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(600),
        spec: spec()?,
        sandbox_id: "sandbox".to_string(),
        cpu_seconds_consumed: 0,
        wall_seconds_consumed: 0,
        correlation_id: None,
        tags: SandboxTags::default(),
        name: None,
        worker_id: worker.cloned(),
        dispatch_attempt: None,
        quota_reservation: None,
        last_activity_at: None,
        paused_remaining_seconds: None,
        freeze_in_flight: freeze,
        auto_paused_at: None,
        idle_pause_retry: None,
        dormancy: None,
        restored_from: None,
    })
}

async fn set_state(
    js: &jetstream::Context,
    worker_id: &WorkerId,
    state: WorkerPlacementState,
    reason: Option<&str>,
) -> Result<Buffers, Box<dyn Error>> {
    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        worker::set_state(
            js.clone(),
            constants::DEFAULT_ACTIVE_LEASE_BUCKET,
            ProviderKind::LocalDocker,
            worker_id,
            state,
            reason.map(str::to_string),
            &mut console,
        )
        .await?;
    }
    Ok(buffers)
}

#[tokio::test]
async fn drain_reports_the_leases_left_and_uncordon_returns_the_worker_to_service() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let worker_id: WorkerId = "w1".parse()?;
    let active = NatsKvActiveLeaseStore::new(js.clone(), constants::DEFAULT_ACTIVE_LEASE_BUCKET).await?;
    active
        .put(&lease_on("held", ProviderKind::LocalDocker, Some(&worker_id), None)?)
        .await?;
    active
        .put(&lease_on("elsewhere", ProviderKind::LocalDocker, None, None)?)
        .await?;

    let draining = set_state(&js, &worker_id, WorkerPlacementState::Draining, Some("kernel patch")).await?;
    let out = draining.out();
    assert!(out.contains("docker w1: draining"), "{out}");
    assert!(out.contains("state: draining"), "{out}");
    assert!(out.contains("remaining leases: 1"), "{out}");
    assert!(out.contains("held"), "{out}");
    assert!(!out.contains("elsewhere"), "{out}");
    assert!(!out.contains("drained: no live leases"), "{out}");

    active.delete(&LeaseId::new("held")).await?;
    let mut status = Buffers::default();
    {
        let mut console = Console::new(&mut status.out, &mut status.err);
        worker::status(
            js.clone(),
            constants::DEFAULT_ACTIVE_LEASE_BUCKET,
            ProviderKind::LocalDocker,
            &worker_id,
            &mut console,
        )
        .await?;
    }
    assert!(status.out().contains("drained: no live leases"), "{}", status.out());

    let mut listed = Buffers::default();
    {
        let mut console = Console::new(&mut listed.out, &mut listed.err);
        worker::list(js.clone(), &mut console).await?;
    }
    assert!(
        listed.out().contains("docker w1 draining (kernel patch)"),
        "{}",
        listed.out()
    );

    let uncordoned = set_state(&js, &worker_id, WorkerPlacementState::Active, None).await?;
    assert!(uncordoned.out().contains("docker w1: active"), "{}", uncordoned.out());
    assert!(!uncordoned.out().contains("remaining leases"), "{}", uncordoned.out());

    let mut after = Buffers::default();
    {
        let mut console = Console::new(&mut after.out, &mut after.err);
        worker::status(
            js,
            constants::DEFAULT_ACTIVE_LEASE_BUCKET,
            ProviderKind::LocalDocker,
            &worker_id,
            &mut console,
        )
        .await?;
    }
    assert!(after.out().contains("state: active"), "{}", after.out());
    Ok(())
}

fn marker(attempts: u32) -> FreezeInFlight {
    FreezeInFlight {
        action: FreezeAction::Resume,
        started_at: SystemTime::now() - Duration::from_secs(3600),
        settle_attempts: attempts,
        next_settle_at: Some(SystemTime::now() + Duration::from_secs(3600)),
    }
}

async fn freeze(
    js: &jetstream::Context,
    id: &str,
    resolution: FreezeResolution,
    force: bool,
) -> Result<Result<(), AdminError>, Box<dyn Error>> {
    let active = NatsKvActiveLeaseStore::new(js.clone(), constants::DEFAULT_ACTIVE_LEASE_BUCKET).await?;
    let idempotency = NatsKvIdempotencyStore::new(js.clone(), constants::DEFAULT_IDEMPOTENCY_BUCKET).await?;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = lease::freeze(
        &active,
        &idempotency,
        &LeaseId::new(id),
        resolution,
        force,
        &mut console,
    )
    .await;
    if result.is_ok() {
        assert!(!buffers.out.is_empty());
    }
    Ok(result)
}

#[tokio::test]
async fn lease_freeze_clears_retries_and_refuses_through_the_stores() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let active = NatsKvActiveLeaseStore::new(js.clone(), constants::DEFAULT_ACTIVE_LEASE_BUCKET).await?;
    active
        .put(&lease_on(
            "stuck",
            ProviderKind::Mock,
            None,
            Some(marker(FREEZE_SETTLE_MAX_ATTEMPTS)),
        )?)
        .await?;
    active
        .put(&lease_on("busy", ProviderKind::Mock, None, Some(marker(2)))?)
        .await?;
    active.put(&lease_on("clean", ProviderKind::Mock, None, None)?).await?;

    freeze(&js, "stuck", FreezeResolution::Retry, false).await??;
    let record = active.get(&LeaseId::new("stuck")).await?.ok_or("missing lease")?;
    let retried = record.freeze_in_flight.ok_or("marker dropped by retry")?;
    assert_eq!((retried.settle_attempts, retried.next_settle_at), (0, None));

    assert!(matches!(
        freeze(&js, "busy", FreezeResolution::Clear, false).await?,
        Err(AdminError::FreezeStillSettling { settle_attempts: 2 })
    ));
    freeze(&js, "busy", FreezeResolution::Clear, true).await??;
    let record = active.get(&LeaseId::new("busy")).await?.ok_or("missing lease")?;
    assert_eq!(record.freeze_in_flight, None);

    freeze(&js, "clean", FreezeResolution::Clear, false).await??;
    freeze(&js, "absent", FreezeResolution::Clear, false).await??;
    Ok(())
}

#[tokio::test]
async fn lease_freeze_reports_each_outcome_on_stdout() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let active = NatsKvActiveLeaseStore::new(js.clone(), constants::DEFAULT_ACTIVE_LEASE_BUCKET).await?;
    let idempotency = NatsKvIdempotencyStore::new(js, constants::DEFAULT_IDEMPOTENCY_BUCKET).await?;
    active
        .put(&lease_on(
            "stuck",
            ProviderKind::Mock,
            None,
            Some(marker(FREEZE_SETTLE_MAX_ATTEMPTS)),
        )?)
        .await?;
    active.put(&lease_on("clean", ProviderKind::Mock, None, None)?).await?;

    for (id, expected) in [
        ("stuck", "applied"),
        ("clean", "no freeze marker"),
        ("absent", "no such lease"),
    ] {
        let mut buffers = Buffers::default();
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        lease::freeze(
            &active,
            &idempotency,
            &LeaseId::new(id),
            FreezeResolution::Clear,
            false,
            &mut console,
        )
        .await?;
        assert_eq!(buffers.out().trim(), expected, "{id}");
    }
    Ok(())
}

fn heartbeat(
    provider: ProviderKind,
    worker: &str,
    profile: ProviderCapabilityProfile,
) -> Result<WorkerHeartbeat, Box<dyn Error>> {
    Ok(WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker.parse()?,
        provider,
        capacity: WorkerCapacity {
            max_concurrent_leases: 4,
            current_leases: 0,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: Default::default(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    })
}

/// Publishes a heartbeat every so often until dropped, so one lands inside the command's window.
fn beat(client: async_nats::Client, heartbeat: WorkerHeartbeat) -> Result<tokio::task::JoinHandle<()>, Box<dyn Error>> {
    let subject = worker_heartbeat(heartbeat.provider, &heartbeat.worker_id);
    let payload = Bytes::from(serde_json::to_vec(&heartbeat)?);
    Ok(tokio::spawn(async move {
        loop {
            let _ = client.publish(subject.clone(), payload.clone()).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }))
}

fn profile(provider: ProviderKind, privileged: CapabilitySupport) -> ProviderCapabilityProfile {
    let mut profile = default_capability_profile();
    profile.provider = provider;
    profile.isolation.supports_privileged = privileged;
    profile
}

#[tokio::test]
async fn capability_show_prints_the_profile_from_a_live_heartbeat() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;

    let mut silent = Buffers::default();
    {
        let mut console = Console::new(&mut silent.out, &mut silent.err);
        capability::show(&client, ProviderKind::LocalDocker, &mut console).await?;
    }
    assert!(silent.out().contains("no live worker heartbeat"), "{}", silent.out());

    let beating = beat(
        client.clone(),
        heartbeat(
            ProviderKind::LocalDocker,
            "docker-1",
            profile(ProviderKind::LocalDocker, CapabilitySupport::Supported),
        )?,
    )?;
    let mut shown = Buffers::default();
    {
        let mut console = Console::new(&mut shown.out, &mut shown.err);
        capability::show(&client, ProviderKind::LocalDocker, &mut console).await?;
    }
    beating.abort();
    let parsed: ProviderCapabilityProfile = serde_json::from_str(&shown.out())?;
    assert_eq!(parsed, profile(ProviderKind::LocalDocker, CapabilitySupport::Supported));
    Ok(())
}

#[tokio::test]
async fn capability_diff_lists_only_the_fields_that_differ() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let docker = beat(
        client.clone(),
        heartbeat(
            ProviderKind::LocalDocker,
            "docker-1",
            profile(ProviderKind::LocalDocker, CapabilitySupport::Supported),
        )?,
    )?;
    let subprocess = beat(
        client.clone(),
        heartbeat(
            ProviderKind::Subprocess,
            "sub-1",
            profile(ProviderKind::Subprocess, CapabilitySupport::Unsupported),
        )?,
    )?;

    let mut buffers = Buffers::default();
    let result = {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        capability::diff(
            &client,
            ProviderKind::LocalDocker,
            ProviderKind::Subprocess,
            &mut console,
        )
        .await
    };
    docker.abort();
    subprocess.abort();
    result?;
    let lines: Vec<String> = buffers.out().lines().map(str::to_string).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], "comparing localdocker vs subprocess");
    assert!(lines[1].starts_with("isolation.supports_privileged: "), "{}", lines[1]);
    Ok(())
}

#[tokio::test]
async fn capability_diff_needs_a_live_worker_for_both_providers() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let docker = beat(
        client.clone(),
        heartbeat(
            ProviderKind::LocalDocker,
            "docker-1",
            profile(ProviderKind::LocalDocker, CapabilitySupport::Supported),
        )?,
    )?;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = capability::diff(
        &client,
        ProviderKind::LocalDocker,
        ProviderKind::Subprocess,
        &mut console,
    )
    .await;
    docker.abort();
    assert!(matches!(result, Err(AdminError::NoLiveWorker { .. })));
    Ok(())
}

/// A registry that answers one request: records its first line and replies with `status`.
async fn fake_registry(status: &'static str) -> Result<(String, tokio::task::JoinHandle<String>), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let served = tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return String::new();
        };
        let mut seen = Vec::new();
        let mut chunk = [0u8; 1024];
        while !seen.windows(4).any(|window| window == b"\r\n\r\n") {
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(read) => seen.extend_from_slice(&chunk[..read]),
            }
        }
        let reply = format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        let _ = socket.write_all(reply.as_bytes()).await;
        let _ = socket.shutdown().await;
        String::from_utf8_lossy(&seen)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    });
    Ok((address, served))
}

fn delete_request(address: &str) -> Result<SnapshotDeleteRequest, Box<dyn Error>> {
    Ok(SnapshotDeleteRequest {
        registry: address.parse::<RegistryHost>()?,
        insecure_registry: true,
        tenant: tenant()?,
        digest: digest()?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
    })
}

#[tokio::test]
async fn snapshot_delete_removes_the_manifest_by_digest_from_the_registry() -> TestResult {
    let (address, served) = fake_registry("202 Accepted").await?;
    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        snapshot::delete(delete_request(&address)?, &mut console).await?;
    }
    let request_line = served.await?;
    assert!(request_line.starts_with("DELETE /v2/"), "{request_line}");
    assert!(
        request_line.contains(&format!("/manifests/{}", digest()?.as_str())),
        "{request_line}"
    );
    assert!(buffers.err().contains("deleted "), "{}", buffers.err());
    assert!(buffers.err().contains(digest()?.as_str()), "{}", buffers.err());
    Ok(())
}

#[tokio::test]
async fn snapshot_delete_surfaces_a_registry_refusal() -> TestResult {
    let (address, _served) = fake_registry("404 Not Found").await?;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = snapshot::delete(delete_request(&address)?, &mut console).await;
    assert!(matches!(
        result,
        Err(AdminError::Step {
            context: "delete snapshot artifact",
            ..
        })
    ));
    assert!(buffers.err().is_empty());
    Ok(())
}

fn prebuild_request(
    endpoint: &str,
    image: &str,
    bearer: std::path::PathBuf,
) -> Result<PrebuildRequest, Box<dyn Error>> {
    Ok(PrebuildRequest {
        ate_api_endpoint: endpoint.to_string(),
        tls_ca_file: None,
        tls_server_name: None,
        bearer_token_file: Some(bearer),
        tenant: tenant()?,
        image: image.to_string(),
        sandbox_class: "gvisor".parse()?,
        agent_image: format!("registry.example/agent@sha256:{}", "0".repeat(64)),
        agent_port: 8110,
        size_class_label: "small".to_string(),
        snapshots_location: String::new(),
        actor_cpu_millicores: 1000,
        memory_mib: 512,
        timeout: Duration::from_secs(5),
    })
}

fn pinned_image() -> String {
    format!("registry.example/app@sha256:{}", "1".repeat(64))
}

#[tokio::test]
async fn substrate_prebuild_refuses_an_image_that_is_not_pinned_by_digest() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let dir = tempfile::tempdir()?;
    let token = dir.path().join("token");
    std::fs::write(&token, "token")?;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = substrate::prebuild(
        js,
        prebuild_request("http://127.0.0.1:1", "registry.example/app:latest", token)?,
        &mut console,
    )
    .await;
    assert!(
        matches!(
            result,
            Err(AdminError::Step {
                context: "validate image",
                ..
            })
        ),
        "{result:?}"
    );
    assert!(buffers.out.is_empty());
    Ok(())
}

#[tokio::test]
async fn substrate_prebuild_reports_an_unreachable_control_plane() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let dir = tempfile::tempdir()?;
    let token = dir.path().join("token");
    std::fs::write(&token, "token")?;
    let mut buffers = Buffers::default();
    let mut console = Console::new(&mut buffers.out, &mut buffers.err);
    let result = substrate::prebuild(
        js,
        prebuild_request("http://127.0.0.1:1", &pinned_image(), token)?,
        &mut console,
    )
    .await;
    assert!(
        matches!(
            result,
            Err(AdminError::Step {
                context: "reach the substrate control plane",
                ..
            })
        ),
        "{result:?}"
    );
    assert!(buffers.out.is_empty());
    Ok(())
}

/// Builds a template against a real Agent Substrate control plane. Set
/// `SANDBOX_ADMIN_LIVE_SUBSTRATE_ENDPOINT` to its API endpoint,
/// `SANDBOX_ADMIN_LIVE_SUBSTRATE_IMAGE` to a digest-pinned image it can pull,
/// `SANDBOX_ADMIN_LIVE_SUBSTRATE_AGENT_IMAGE` to the digest-pinned actor agent image and
/// `SANDBOX_ADMIN_LIVE_SUBSTRATE_SIZE_CLASS` to the pool's `ate.dev/size` label.
#[tokio::test]
#[ignore = "needs a live Agent Substrate control plane: set SANDBOX_ADMIN_LIVE_SUBSTRATE_ENDPOINT"]
async fn substrate_prebuild_builds_a_template_on_a_live_control_plane() -> TestResult {
    let need = |name: &str| {
        SystemEnv
            .var(name)
            .map_err(|_| format!("{name} is required for this test"))
    };
    let endpoint = need("SANDBOX_ADMIN_LIVE_SUBSTRATE_ENDPOINT")?;
    let image = need("SANDBOX_ADMIN_LIVE_SUBSTRATE_IMAGE")?;
    let agent_image = need("SANDBOX_ADMIN_LIVE_SUBSTRATE_AGENT_IMAGE")?;
    let size_class = need("SANDBOX_ADMIN_LIVE_SUBSTRATE_SIZE_CLASS")?;

    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let dir = tempfile::tempdir()?;
    let token = dir.path().join("token");
    std::fs::write(
        &token,
        SystemEnv.var("SANDBOX_ADMIN_LIVE_SUBSTRATE_TOKEN").unwrap_or_default(),
    )?;
    let mut request = prebuild_request(&endpoint, &image, token)?;
    request.agent_image = agent_image;
    request.size_class_label = size_class;
    request.timeout = Duration::from_secs(600);

    let mut buffers = Buffers::default();
    {
        let mut console = Console::new(&mut buffers.out, &mut buffers.err);
        substrate::prebuild(js, request, &mut console).await?;
    }
    let report: serde_json::Value = serde_json::from_str(buffers.out().trim())?;
    assert_eq!(report["image"], image);
    assert!(report["template"].as_str().is_some_and(|name| !name.is_empty()));
    Ok(())
}
