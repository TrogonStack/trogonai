//! The whole release path over a real NATS server and the real snapshot builder binary: the
//! broker grants a lease to this worker, releases it, the builder captures and publishes a
//! protected snapshot, and the broker restores and forks new leases from it.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, bail, ensure};
use futures::StreamExt;
use trogon_sandbox_broker::active_lease::InMemoryActiveLeaseStore;
use trogon_sandbox_broker::core::{BrokerConfig, BrokerCore, InMemoryDriveStore};
use trogon_sandbox_broker::digest_pinning::InMemorySnapshotPinStore;
use trogon_sandbox_broker::host_import_policy::InMemoryHostImportPolicy;
use trogon_sandbox_broker::host_mount_policy::InMemoryHostMountAllowList;
use trogon_sandbox_broker::idempotency::InMemoryIdempotencyStore;
use trogon_sandbox_broker::image_policy::InMemoryImageAllowList;
use trogon_sandbox_broker::limits::InMemoryTenantLimits;
use trogon_sandbox_broker::metrics::BrokerMetrics;
use trogon_sandbox_broker::named_lease::InMemoryNamedLeaseStore;
use trogon_sandbox_broker::nats_bus::NatsBus;
use trogon_sandbox_broker::quota::InMemoryQuotaStore;
use trogon_sandbox_broker::resolved_digest::InMemoryResolvedDigestStore;
use trogon_sandbox_broker::snapshot_capture_status::{NatsKvSnapshotCaptureStatusStore, SnapshotCaptureStatusStore};
use trogon_sandbox_broker::snapshot_tag::{InMemorySnapshotTagAuditor, InMemorySnapshotTagStore};
use trogon_sandbox_broker::worker_registry::InMemoryWorkerRegistry;
use trogon_sandbox_protocol::subjects;
use trogon_std::env::{ReadEnv, SystemEnv};

use super::*;

const BUILDER_ENV: &str = "SANDBOX_TEST_SNAPSHOT_BUILDER";
const REAL_SCANNER_ENV: &str = "SANDBOX_TEST_REAL_TRIVY";

/// A child process that is killed and reaped when the test lets go of it, whether it passes or not.
struct OwnedProcess(Child);

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The client URL and monitoring port from a ports file, once the server has fully written it.
/// The server picks its own ports: reserving them here and handing them over races other tests
/// doing the same, and the loser's client silently connects to the winner's server.
fn server_ports(ports_file: &std::path::Path) -> Option<(String, u16)> {
    let ports: serde_json::Value = serde_json::from_slice(&std::fs::read(ports_file).ok()?).ok()?;
    let url = ports["nats"][0].as_str()?.to_string();
    let monitor = ports["monitoring"][0].as_str()?.rsplit(':').next()?.parse().ok()?;
    Some((url, monitor))
}

/// Waits until some client of the server listens on `subject`, read from the monitoring endpoint.
async fn wait_subscription(monitor: u16, subject: &str) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", monitor)).await
                && stream
                    .write_all(b"GET /subsz?subs=1 HTTP/1.0\r\nHost: localhost\r\n\r\n")
                    .await
                    .is_ok()
            {
                let mut response = vec![];
                if stream.take(1024 * 1024).read_to_end(&mut response).await.is_ok()
                    && String::from_utf8_lossy(&response)
                        .replace("\\u003e", ">")
                        .contains(subject)
                {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .with_context(|| format!("no service subscribed to {subject}"))
}

fn broker(
    client: async_nats::Client,
    registry: Arc<InMemoryWorkerRegistry>,
    artifact_registry: &str,
) -> anyhow::Result<BrokerCore> {
    Ok(BrokerCore::new(
        Arc::new(NatsBus::new(client)),
        Arc::new(InMemoryQuotaStore::new()),
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        registry,
        InMemorySnapshotPinStore::new(),
        Arc::new(InMemorySnapshotTagStore::new(Arc::new(
            InMemorySnapshotTagAuditor::new(),
        ))),
        Arc::new(InMemoryResolvedDigestStore::new()),
        Arc::new(InMemoryDriveStore::new()),
        Arc::new(InMemoryNamedLeaseStore::new()),
        BrokerConfig {
            default_cpu_cap: 3600,
            default_wall_cap: 7200,
            enforce_image_allowlist: false,
            enforce_worker_registry: false,
            default_max_cpu_per_request: 3600,
            default_max_memory_mib_per_request: 8192,
            default_active_lease_cap: 8,
            snapshot_registry: artifact_registry.into(),
            provider_dispatch_timeout: Duration::from_secs(20),
        },
        BrokerMetrics::new()?,
    ))
}

fn granted(response: LeaseResponse, step: &str) -> anyhow::Result<LeaseGranted> {
    match response {
        LeaseResponse::Granted(granted) => Ok(granted),
        other => bail!("{step}: {other:?}"),
    }
}

/// A Debian-looking root filesystem image with a package database a scanner can read.
fn base_image(root: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let source = root.join("image-tree");
    std::fs::create_dir_all(source.join("var/lib/dpkg"))?;
    std::fs::create_dir_all(source.join("etc"))?;
    std::fs::write(
        source.join("etc/os-release"),
        "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nNAME=\"Debian GNU/Linux\"\nID=debian\nVERSION_ID=13\nVERSION_CODENAME=trixie\n",
    )?;
    std::fs::write(source.join("etc/debian_version"), "13.0\n")?;
    std::fs::write(
        source.join("var/lib/dpkg/status"),
        "Package: base-files\nStatus: install ok installed\nArchitecture: amd64\nVersion: 13.8\n\n",
    )?;
    let base = root.join("base.ext4");
    std::fs::File::create(&base)?.set_len(32 * 1024 * 1024)?;
    ensure!(
        Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(&source)
            .arg(&base)
            .status()?
            .success(),
        "mkfs.ext4 could not build the base image"
    );
    Ok(base)
}

/// Starts a JetStream-enabled server on ports of its own choosing and connects to it.
async fn nats_server(root: &std::path::Path) -> anyhow::Result<(OwnedProcess, String, u16, async_nats::Client)> {
    let mut child = Command::new("nats-server")
        .args(["-c", "/dev/stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let ports_file = root.join(format!("nats-server_{}.ports", child.id()));
    let stdin = child.stdin.take();
    let server = OwnedProcess(child);
    writeln!(
        stdin.context("nats-server has no stdin")?,
        "host: 127.0.0.1\nport: -1\nhttp: 127.0.0.1:-1\nports_file_dir: {:?}\nmax_payload: 65536\njetstream {{ store_dir: {:?} }}",
        root,
        root.join("nats")
    )?;
    let (url, monitor, client) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some((url, monitor)) = server_ports(&ports_file)
                && let Ok(client) = async_nats::connect(&url).await
            {
                break (url, monitor, client);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("nats-server never accepted a client")?;
    Ok((server, url, monitor, client))
}

/// A scanner stand-in that proves the builder handed it the captured disk's package database
/// and never the captured memory.
fn fake_scanner(tools: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir(tools)?;
    let trivy = tools.join("trivy");
    std::fs::write(
        &trivy,
        "#!/bin/sh\nset -eu\ntest \"$1\" = rootfs\ntest -f \"$2/var/lib/dpkg/status\"\ngrep -q 'Package: base-files' \"$2/var/lib/dpkg/status\"\ntest ! -e \"$2/snapshot.memory\"\nprintf 'package-database-seen' > \"$SANDBOX_TEST_SCANNED\"\nprintf '{\"Results\":[]}'\n",
    )?;
    std::fs::set_permissions(&trivy, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Linux, nats-server, mkfs.ext4, and SANDBOX_TEST_SNAPSHOT_BUILDER pointing to the built trogon-sandbox-snapshot-builder binary"]
async fn broker_release_builder_protected_publication_and_new_lease_restore() -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(90), pipeline())
        .await
        .context("release pipeline deadline")?
}

async fn pipeline() -> anyhow::Result<()> {
    let program = SystemEnv
        .var(BUILDER_ENV)
        .with_context(|| format!("set {BUILDER_ENV} to the built builder binary"))?;
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let launcher = Arc::new(FakeLauncher::default());
    let provider = Arc::new(provider_config(root.path(), &registry, |_| {}, launcher.clone()).await?);
    let expected_disk = hash(&tokio::fs::read(base_image(root.path())?).await?);

    let (_nats, url, monitor, client) = nats_server(root.path()).await?;
    let mut heartbeats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Firecracker))
        .await?;
    let mut published = client.subscribe(subjects::SNAPSHOT_PUBLISHED).await?;
    client.flush().await?;
    let worker_id = provider.dispatch_ledger().await?.worker_id().clone();
    let stop = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(crate::worker::run(
        client.clone(),
        async_nats::jetstream::new(client.clone()),
        provider.clone(),
        worker_id,
        stop.clone(),
        &SystemEnv,
    ));
    let heartbeat = tokio::time::timeout(Duration::from_secs(10), heartbeats.next())
        .await
        .context("the worker never sent a heartbeat")?
        .context("the heartbeat subscription closed")?;
    let workers = InMemoryWorkerRegistry::new(Duration::from_secs(120));
    workers.inject(serde_json::from_slice(&heartbeat.payload)?).await;

    let tools = root.path().join("tools");
    let scanned = root.path().join("scanned");
    let real_scanner = SystemEnv.var(REAL_SCANNER_ENV).as_deref() == Ok("1");
    fake_scanner(&tools)?;
    let mut builder_command = Command::new(program);
    builder_command
        .args([
            "--nats-url",
            &url,
            "--registry-url",
            &registry.address,
            "--oci-insecure-registries",
            &registry.address,
            "--metrics-addr",
            "127.0.0.1:0",
        ])
        .env("SANDBOX_TEST_SCANNED", &scanned)
        .env("RUST_LOG", "info")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if !real_scanner {
        let path = SystemEnv.var("PATH").unwrap_or_default();
        builder_command.env("PATH", format!("{}:{path}", tools.display()));
    }
    let _builder = OwnedProcess(builder_command.spawn()?);
    wait_subscription(monitor, subjects::LEASE_RELEASED_FILTER).await?;

    let broker = broker(client.clone(), workers, &registry.address)?;
    let source = granted(broker.handle_lease_request(request()?).await, "initial lease")?;
    broker
        .handle_cancel(source.lease_id.clone(), LeaseReleasedReason::Cancelled)
        .await?;
    ensure!(
        !provider.holds_lease(&source.lease_id).await,
        "the released lease is still held"
    );
    ensure!(
        !provider
            .config()
            .state_dir
            .as_path()
            .join(source.lease_id.as_str())
            .exists(),
        "the released lease kept its directory"
    );

    let publication = match tokio::time::timeout(Duration::from_secs(30), published.next()).await {
        Ok(Some(event)) => event,
        result => {
            let status = NatsKvSnapshotCaptureStatusStore::new(async_nats::jetstream::new(client.clone())).await?;
            bail!(
                "protected publication failed: {result:?}; capture status: {:?}",
                status.get(&source.lease_id).await?
            );
        }
    };
    let event: SnapshotPublished = serde_json::from_slice(&publication.payload)?;
    assert_eq!(event.lease_id, source.lease_id);
    assert_eq!(event.artifact_type, ArtifactType::FirecrackerSnapshotV1);
    if real_scanner {
        let data = registry.data.lock().await;
        let reports: Vec<serde_json::Value> = data
            .blobs
            .values()
            .filter_map(|blob| serde_json::from_slice(blob).ok())
            .collect();
        ensure!(
            reports
                .iter()
                .any(|report| report["SchemaVersion"] == 2 && report["Metadata"]["OS"]["Family"] == "debian"),
            "publication must attach the scanner's Debian rootfs report; JSON blobs: {reports:?}"
        );
    } else {
        assert_eq!(std::fs::read(&scanned)?, b"package-database-seen");
    }

    let mut restore_parts = crate::adapter::test_support::spec_parts("guest:release-test", 1, 60)?;
    restore_parts.image = SandboxImage::Snapshot {
        reference: OciArtifactRef {
            registry: registry.address.clone(),
            repository: policy().target_repository,
            digest: event.digest,
            artifact_type: event.artifact_type,
        },
        parent: None,
    };
    let restored = granted(
        broker
            .handle_lease_request(lease_request(restore_parts, "restored")?)
            .await,
        "restored lease",
    )?;
    assert_ne!(restored.lease_id, source.lease_id);
    let fork = granted(
        broker
            .handle_fork_request(LeaseForkRequest {
                tenant: request()?.tenant,
                source_lease_id: restored.lease_id.clone(),
                ttl_seconds: 60,
                idempotency_key: "fork-restored".into(),
            })
            .await,
        "fork restored lease",
    )?;
    assert_ne!(fork.lease_id, restored.lease_id);
    assert!(provider.holds_lease(&restored.lease_id).await);
    assert!(provider.holds_lease(&fork.lease_id).await);
    {
        let launches = launcher
            .launches
            .lock()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        assert_eq!(
            launches.as_slice(),
            &[
                (expected_disk.clone(), false),
                (expected_disk.clone(), true),
                (expected_disk, true)
            ]
        );
    }
    let deleted = broker
        .handle_delete_lease(DeleteLeaseRequest {
            tenant: request()?.tenant,
            lease_id: fork.lease_id,
            operation_id: None,
        })
        .await;
    assert!(matches!(deleted, DeleteLeaseResponse::Deleted { .. }), "{deleted:?}");
    assert!(provider.holds_lease(&restored.lease_id).await);
    broker
        .handle_cancel(restored.lease_id, LeaseReleasedReason::Cancelled)
        .await?;
    provider.purge(&source.lease_id).await?;
    stop.cancel();
    worker.await??;
    Ok(())
}
