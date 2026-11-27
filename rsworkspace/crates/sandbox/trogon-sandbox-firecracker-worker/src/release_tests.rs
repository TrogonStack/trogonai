use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use axum::{
    Router,
    body::Body,
    extract::State,
    response::{IntoResponse, Response},
    routing::{any, get, post, put},
};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::*;
use trogon_sandbox_protocol::Provider;

use crate::adapter::test_support::{TestResult, lease_request, spec_parts};
use crate::{
    FirecrackerProvider,
    config::*,
    process::{VmLauncher, VmProcess},
    vm_dir::VmDir,
};
use std::sync::atomic::Ordering;
use std::time::Duration;
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode};

#[derive(Default)]
struct RegistryData {
    next: u64,
    uploads: HashMap<u64, Vec<u8>>,
    blobs: HashMap<String, Vec<u8>>,
    manifests: HashMap<String, Vec<u8>>,
    pushes: usize,
    fail_manifests: bool,
}

struct Registry {
    address: String,
    data: Arc<tokio::sync::Mutex<RegistryData>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn hash(data: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(data)))
}

fn reply(status: u16, location: Option<String>, bytes: Vec<u8>) -> Response {
    let mut builder = Response::builder().status(status);
    if let Some(location) = location {
        builder = builder.header("location", location);
    }
    builder
        .body(Body::from(bytes))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

impl Registry {
    async fn start() -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        let data = Arc::new(tokio::sync::Mutex::new(RegistryData::default()));
        let state = (address.clone(), Arc::clone(&data));
        let router = Router::new().fallback(any(registry_request)).with_state(state);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { address, data, task })
    }
    fn client(&self) -> anyhow::Result<Arc<trogon_sandbox_oci::OciClient>> {
        Ok(Arc::new(trogon_sandbox_oci::OciClient::new(
            trogon_sandbox_oci::OciClientConfig {
                insecure_registries: vec![trogon_sandbox_oci::RegistryHost::new(self.address.clone())?],
                ..Default::default()
            },
        )))
    }
}

async fn registry_request(
    State((address, data)): State<(String, Arc<tokio::sync::Mutex<RegistryData>>)>,
    request: axum::extract::Request,
) -> Response {
    match serve_registry_request(address, data, request).await {
        Ok(response) => response,
        Err(_) => reply(500, None, vec![]),
    }
}

async fn serve_registry_request(
    address: String,
    data: Arc<tokio::sync::Mutex<RegistryData>>,
    request: axum::extract::Request,
) -> anyhow::Result<Response> {
    let method = request.method().as_str().to_string();
    let uri = request.uri().clone();
    let path = uri.path();
    let body = axum::body::to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await?
        .to_vec();
    let mut data = data.lock().await;
    if path == "/v2/" {
        return Ok(reply(200, None, b"{}".to_vec()));
    }
    if path.contains("/referrers/") {
        return Ok(Response::builder()
            .status(200)
            .header("content-type", "application/vnd.oci.image.index.v1+json")
            .body(Body::from(
                r#"{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[]}"#,
            ))?);
    }
    if let Some((repository, id)) = path.split_once("/blobs/uploads/") {
        if method == "POST" {
            data.next += 1;
            let id = data.next;
            data.uploads.insert(id, Vec::new());
            return Ok(reply(
                202,
                Some(format!("http://{address}{repository}/blobs/uploads/{id}")),
                vec![],
            ));
        }
        let Ok(id) = id.parse::<u64>() else {
            return Ok(reply(404, None, vec![]));
        };
        if method == "PATCH" {
            data.uploads
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("unknown upload"))?
                .extend(body);
            return Ok(reply(
                202,
                Some(format!("http://{address}{repository}/blobs/uploads/{id}")),
                vec![],
            ));
        }
        if method == "PUT" {
            let digest = uri
                .query()
                .unwrap_or_default()
                .split('&')
                .find_map(|pair| pair.strip_prefix("digest="))
                .unwrap_or_default()
                .replace("%3A", ":")
                .replace("%3a", ":");
            let mut content = data
                .uploads
                .remove(&id)
                .ok_or_else(|| anyhow::anyhow!("unknown upload"))?;
            content.extend(body);
            anyhow::ensure!(hash(&content) == digest, "uploaded content must match its digest");
            data.blobs.insert(digest.clone(), content);
            return Ok(reply(
                201,
                Some(format!("http://{address}{repository}/blobs/{digest}")),
                vec![],
            ));
        }
    }
    if let Some((repository, tag)) = path.split_once("/manifests/") {
        if method == "PUT" {
            if data.fail_manifests {
                return Ok(reply(500, None, vec![]));
            }
            let digest = hash(&body);
            data.manifests.insert(format!("{repository}/{tag}"), body.clone());
            data.manifests.insert(format!("{repository}/{digest}"), body);
            data.pushes += 1;
            return Ok(reply(
                201,
                Some(format!("http://{address}{repository}/manifests/{digest}")),
                vec![],
            ));
        }
        if let Some(body) = data.manifests.get(&format!("{repository}/{tag}")) {
            let content_type = if serde_json::from_slice::<serde_json::Value>(body)?
                .get("manifests")
                .is_some()
            {
                "application/vnd.oci.image.index.v1+json"
            } else {
                "application/vnd.oci.image.manifest.v1+json"
            };
            return Ok(Response::builder()
                .status(200)
                .header("content-type", content_type)
                .header("docker-content-digest", hash(body))
                .body(Body::from(body.clone()))?);
        }
    }
    if let Some((_, digest)) = path.split_once("/blobs/")
        && let Some(body) = data.blobs.get(digest)
    {
        return Ok(reply(200, None, if method == "HEAD" { vec![] } else { body.clone() }));
    }
    Ok(reply(404, None, vec![]))
}

#[derive(Default)]
struct FakeLauncher {
    exec_calls: Arc<std::sync::atomic::AtomicUsize>,
    clock_source: Option<&'static str>,
    tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    capture: Option<Arc<CaptureGate>>,
    launches: StdMutex<Vec<(String, bool)>>,
}

#[derive(Default)]
struct CaptureGate {
    entered: tokio::sync::Notify,
    proceed: tokio::sync::Notify,
    count: std::sync::atomic::AtomicUsize,
    fail: std::sync::atomic::AtomicBool,
}

impl Drop for FakeLauncher {
    fn drop(&mut self) {
        if let Ok(tasks) = self.tasks.get_mut() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
    }
}

#[async_trait::async_trait]
impl VmLauncher for FakeLauncher {
    async fn launch(&self, dir: &VmDir) -> Result<VmProcess, crate::error::ProviderError> {
        let fail = |error: &dyn std::fmt::Display| crate::error::ProviderError::Launch(error.to_string());
        let disk_hash = hash(&tokio::fs::read(dir.rootfs()).await.map_err(|error| fail(&error))?);
        self.launches
            .lock()
            .map_err(|error| fail(&error))?
            .push((disk_hash, dir.runtime_dir().join(crate::snapshot::STATE_FILE).exists()));
        let api = tokio::net::UnixListener::bind(dir.api_socket()).map_err(|error| fail(&error))?;
        let root = dir.runtime_dir().to_path_buf();
        let capture = self.capture.clone();
        let router = Router::new()
            .route(
                "/snapshot/create",
                put(
                    |State((root, capture)): State<(std::path::PathBuf, Option<Arc<CaptureGate>>)>,
                     axum::Json(body): axum::Json<serde_json::Value>| async move {
                        if let Some(gate) = &capture {
                            gate.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            gate.entered.notify_one();
                            gate.proceed.notified().await;
                            if gate.fail.swap(false, std::sync::atomic::Ordering::SeqCst) {
                                return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                            }
                        }
                        let (Some(state_path), Some(memory_path)) =
                            (body["snapshot_path"].as_str(), body["mem_file_path"].as_str())
                        else {
                            return axum::http::StatusCode::BAD_REQUEST;
                        };
                        if tokio::fs::write(root.join(state_path), b"vmm-state").await.is_err()
                            || tokio::fs::write(root.join(memory_path), vec![7; 1024 * 1024])
                                .await
                                .is_err()
                        {
                            return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                        }
                        axum::http::StatusCode::NO_CONTENT
                    },
                ),
            )
            .fallback(any(|| async { axum::http::StatusCode::NO_CONTENT }))
            .with_state((root, capture));
        self.tasks
            .lock()
            .map_err(|error| fail(&error))?
            .push(tokio::spawn(async move {
                let _ = axum::serve(api, router).await;
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| fail(&error))?;
        let address = listener.local_addr().map_err(|error| fail(&error))?;
        let clock_source = self.clock_source.unwrap_or("kvm-clock");
        let exec_calls = self.exec_calls.clone();
        let router = Router::new()
            .route("/readyz", get(|| async { axum::http::StatusCode::OK }))
            .route(
                trogon_sandbox_actor_agent::BIND_PATH,
                post(
                    |axum::Json(request): axum::Json<trogon_sandbox_actor_agent::BindLeaseRequest>| async move {
                        if let Some(seed) = request.entropy_seed {
                            let Ok(seed_digest) = Digest::new(hash(seed.as_bytes())) else {
                                return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
                            };
                            axum::Json(trogon_sandbox_actor_agent::BindLeaseResponse {
                                lease_id: request.lease_id,
                                entropy_reseed: trogon_sandbox_actor_agent::GuestEntropyReseedReceipt {
                                    seed_digest,
                                    method: trogon_sandbox_actor_agent::GuestEntropyReseedMethod::LinuxKernelV1,
                                },
                            })
                            .into_response()
                        } else {
                            axum::http::StatusCode::NO_CONTENT.into_response()
                        }
                    },
                ),
            )
            .route(
                trogon_sandbox_actor_agent::EXEC_PATH,
                post(move |axum::Json(request): axum::Json<RunCommandRequest>| {
                    let exec_calls = exec_calls.clone();
                    async move {
                        exec_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let stdout = if request.command
                            == [
                                "/bin/cat",
                                "/sys/devices/system/clocksource/clocksource0/current_clocksource",
                            ] {
                            format!("{clock_source}\n").into_bytes()
                        } else {
                            Vec::new()
                        };
                        serde_json::to_string(&RunCommandResponse::Completed(CommandOutput {
                            exit_code: 0,
                            stdout,
                            stderr: vec![],
                            ..Default::default()
                        }))
                        .map(|body| body + "\n")
                        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }),
            )
            .layer(axum::middleware::map_response(
                |mut response: axum::response::Response| async move {
                    response.headers_mut().insert(
                        trogon_sandbox_actor_agent::AGENT_PROTOCOL_VERSION_HEADER,
                        trogon_sandbox_actor_agent::GUEST_PROTOCOL_VERSION.into(),
                    );
                    response
                },
            ));
        self.tasks
            .lock()
            .map_err(|error| fail(&error))?
            .push(tokio::spawn(async move {
                let _ = axum::serve(listener, router).await;
            }));
        let vsock = tokio::net::UnixListener::bind(dir.vsock_socket()).map_err(|error| fail(&error))?;
        self.tasks
            .lock()
            .map_err(|error| fail(&error))?
            .push(tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = vsock.accept().await else {
                        return;
                    };
                    tokio::spawn(async move {
                        let mut line = [0u8; 13];
                        if stream.read_exact(&mut line).await.is_err() || &line != b"CONNECT 8110\n" {
                            return;
                        }
                        if stream.write_all(b"OK 123\n").await.is_err() {
                            return;
                        }
                        let Ok(mut upstream) = tokio::net::TcpStream::connect(address).await else {
                            return;
                        };
                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
                    });
                }
            }));
        Ok(VmProcess::from_child(
            tokio::process::Command::new("sleep")
                .arg("60")
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| fail(&error))?,
        ))
    }
}

fn policy() -> SnapshotPolicy {
    SnapshotPolicy {
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
        target_repository: "snapshots/release-test".into(),
        retention: None,
        prewarm: None,
        tag_name: None,
    }
}

fn request_out(snapshot_out: Option<SnapshotPolicy>) -> anyhow::Result<LeaseRequest> {
    let mut parts = spec_parts("guest:release-test", 1, 60)?;
    parts.snapshot_out = snapshot_out;
    lease_request(parts, "release")
}

fn request() -> anyhow::Result<LeaseRequest> {
    request_out(Some(policy()))
}

async fn provider(root: &std::path::Path, registry: &Registry) -> anyhow::Result<FirecrackerProvider> {
    provider_config(root, registry, |_| {}, Arc::new(FakeLauncher::default())).await
}

async fn provider_config(
    root: &std::path::Path,
    registry: &Registry,
    edit: impl FnOnce(&mut FirecrackerConfig),
    launcher: Arc<dyn VmLauncher>,
) -> anyhow::Result<FirecrackerProvider> {
    let base = root.join("base.ext4");
    tokio::fs::write(&base, b"disk-proof").await?;
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new("unused"),
        KernelImage::new("unused"),
        BaseRootfs::new(base),
        StateDir::new(root.join("state")),
    );
    config.default_image = OciImageRef::parse_unqualified("guest:release-test");
    config.default_memory = MemoryMib::new(1)?;
    edit(&mut config);
    Ok(FirecrackerProvider::new(config, launcher).with_oci_client(registry.client()?))
}

#[tokio::test]
async fn unsupported_launch_cancellation_is_refused_before_native_api_calls() -> anyhow::Result<()> {
    let nats = trogon_nats::test_support::JetStreamTestServer::start().await;
    let client = nats.client().await;
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let launcher = Arc::new(FakeLauncher::default());
    let provider = Arc::new(provider_config(root.path(), &registry, |_| {}, launcher.clone()).await?);
    let lease = LeaseId::new("launch-refusal");
    let sibling = LeaseId::new("unrelated-sentinel");
    let lease_request = request_out(None)?;
    provider.lease(&lease_request, &lease).await?;
    provider.lease(&lease_request, &sibling).await?;
    let worker_id = provider.dispatch_ledger().await?.worker_id().clone();
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(crate::worker::run_with_authority(
        client.clone(),
        async_nats::jetstream::new(client.clone()),
        provider.clone(),
        worker_id,
        stop.clone(),
        AuthorityConfig::new(AuthorityMode::Off),
    ));
    let observed = async {
        let subject = format!("sandbox.provider.firecracker.run_command.{}", lease.as_str());
        let ordinary: RunCommandRequest = serde_json::from_value(serde_json::json!({
            "lease_id":lease.as_str(), "command":["true"], "timeout_seconds":1,
        }))?;
        let body = serde_json::to_vec(&ordinary)?;
        let calls_before_control = launcher.exec_calls.load(Ordering::SeqCst);
        let control = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(reply) = client.request(subject.clone(), body.clone().into()).await {
                    break serde_json::from_slice::<RunCommandResponse>(&reply.payload);
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await??;
        anyhow::ensure!(
            matches!(control, RunCommandResponse::Completed(ref output) if output.exit_code == 0)
                && launcher.exec_calls.load(Ordering::SeqCst) == calls_before_control + 1,
            "ordinary command must reach the real worker/vsock dispatch: {control:?}"
        );
        let calls_before = launcher.exec_calls.load(Ordering::SeqCst);
        let mut strong = ordinary;
        strong.require_launch_cancellation = LaunchCancellation::Required;
        strong.launch_key = Some(LaunchKey::new("unsupported-strong-request")?);
        let reply = client.request(subject, serde_json::to_vec(&strong)?.into()).await?;
        let response: RunCommandResponse = serde_json::from_slice(&reply.payload)?;
        let calls_after = launcher.exec_calls.load(Ordering::SeqCst);
        let sibling_retained = provider.holds_lease(&sibling).await;
        Ok::<_, anyhow::Error>((response, calls_before, calls_after, sibling_retained))
    }
    .await;
    provider.cancel(&lease).await?;
    provider.cancel(&sibling).await?;
    stop.cancel();
    task.await??;
    anyhow::ensure!(
        !provider.holds_lease(&lease).await
            && !provider.holds_lease(&sibling).await
            && !provider.config().state_dir.as_path().join(lease.as_str()).exists()
            && !provider.config().state_dir.as_path().join(sibling.as_str()).exists(),
        "owned fake-VM processes and directories must be disposed before assertion"
    );
    let (response, calls_before, calls_after, sibling_retained) = observed?;
    anyhow::ensure!(
        matches!(&response, RunCommandResponse::Denied(LeaseError::InvalidSpec { reason }) if reason.contains("launch cancellation"))
            && calls_before == calls_after
            && sibling_retained,
        "unsupported strong opt-in must refuse before a guest exec call: response={response:?}, calls_before={calls_before}, calls_after={calls_after}, sibling_retained={sibling_retained}; ordinary vsock dispatch and owned fake-VM cleanup controls passed"
    );
    Ok(())
}

#[tokio::test]
async fn a_released_lease_remains_exportable_after_its_vmm_and_directory_are_gone() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let provider = provider(root.path(), &registry).await?;
    let lease = LeaseId::new("released");
    provider.lease(&request()?, &lease).await?;
    provider.cancel(&lease).await?;
    assert!(!provider.holds_lease(&lease).await);
    assert!(!provider.config().state_dir.as_path().join(lease.as_str()).exists());
    let target = SnapshotPushTarget {
        registry: registry.address.clone(),
        repository: policy().target_repository,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    };
    let receipt = provider.snapshot_to(&lease, &target).await?;
    assert!(receipt.bytes > 0);
    assert_eq!(registry.data.lock().await.pushes, 2);
    let again = provider.snapshot_to(&lease, &target).await?;
    assert_eq!(receipt, again);
    assert_eq!(registry.data.lock().await.pushes, 2);
    Ok(())
}

#[tokio::test]
async fn unsupported_snapshot_output_is_refused_before_allocating_a_vm() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let provider = provider(root.path(), &registry).await?;
    let request = request_out(Some(SnapshotPolicy {
        artifact_type: ArtifactType::SandboxRootfsV1,
        ..policy()
    }))?;
    assert!(provider.lease(&request, &LeaseId::new("wrong-output")).await.is_err());
    assert_eq!(provider.current_lease_count().await, 0);
    assert!(!provider.config().state_dir.as_path().join("wrong-output").exists());
    Ok(())
}

#[tokio::test]
async fn failed_export_keeps_its_original_target_across_worker_restart() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let lease = LeaseId::new("pinned-target");
    let original = provider(root.path(), &registry).await?;
    original.lease(&request()?, &lease).await?;
    original.cancel(&lease).await?;
    registry.data.lock().await.fail_manifests = true;
    assert!(original.snapshot_to(&lease, &target(&registry)).await.is_err());
    drop(original);
    let other = Registry::start().await?;
    let restarted = provider(root.path(), &other).await?;
    assert!(
        restarted.snapshot_to(&lease, &target(&other)).await.is_err(),
        "a failed push must retain its exact export target"
    );
    assert_eq!(other.data.lock().await.pushes, 0);
    restarted.purge(&lease).await?;
    Ok(())
}

fn target(registry: &Registry) -> SnapshotPushTarget {
    SnapshotPushTarget {
        registry: registry.address.clone(),
        repository: policy().target_repository,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    }
}

#[tokio::test]
async fn retained_exports_and_exact_receipts_survive_worker_restart() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let lease = LeaseId::new("restart");
    let original = provider(root.path(), &registry).await?;
    original.lease(&request()?, &lease).await?;
    original.cancel(&lease).await?;
    drop(original);
    let restarted = provider(root.path(), &registry).await?;
    let receipt = restarted.snapshot_to(&lease, &target(&registry)).await?;
    assert!(restarted.checkpoint_to(&lease, &target(&registry)).await.is_err());
    drop(restarted);
    let again = provider(root.path(), &registry).await?;
    assert_eq!(again.snapshot_to(&lease, &target(&registry)).await?, receipt);
    assert_eq!(registry.data.lock().await.pushes, 2);
    again.purge(&lease).await?;
    assert!(again.snapshot_to(&lease, &target(&registry)).await.is_err());
    Ok(())
}

#[tokio::test]
async fn release_and_export_race_share_one_capture_and_purge_discards_the_receipt() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let capture = Arc::new(CaptureGate::default());
    let launcher = Arc::new(FakeLauncher {
        exec_calls: Default::default(),
        capture: Some(capture.clone()),
        clock_source: None,
        tasks: Default::default(),
        launches: Default::default(),
    });
    let provider = Arc::new(provider_config(root.path(), &registry, |_| {}, launcher).await?);
    let lease = LeaseId::new("race");
    provider.lease(&request()?, &lease).await?;
    let cancel = {
        let provider = provider.clone();
        let lease = lease.clone();
        tokio::spawn(async move { provider.cancel(&lease).await })
    };
    capture.entered.notified().await;
    let export = {
        let provider = provider.clone();
        let lease = lease.clone();
        let target = target(&registry);
        tokio::spawn(async move { provider.snapshot_to(&lease, &target).await })
    };
    capture.proceed.notify_one();
    cancel.await??;
    export.await??;
    assert_eq!(capture.count.load(std::sync::atomic::Ordering::SeqCst), 1);
    provider.purge(&lease).await?;
    assert!(provider.snapshot_to(&lease, &target(&registry)).await.is_err());
    Ok(())
}

#[tokio::test]
async fn failed_capture_keeps_the_source_live_for_a_retry() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let capture = Arc::new(CaptureGate::default());
    capture.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    let provider = Arc::new(
        provider_config(
            root.path(),
            &registry,
            |_| {},
            Arc::new(FakeLauncher {
                exec_calls: Default::default(),
                capture: Some(capture.clone()),
                clock_source: None,
                tasks: Default::default(),
                launches: Default::default(),
            }),
        )
        .await?,
    );
    let lease = LeaseId::new("retry");
    provider.lease(&request()?, &lease).await?;
    capture.proceed.notify_one();
    assert!(provider.cancel(&lease).await.is_err());
    assert!(provider.holds_lease(&lease).await);
    capture.proceed.notify_one();
    provider.cancel(&lease).await?;
    assert!(!provider.holds_lease(&lease).await);
    provider.snapshot_to(&lease, &target(&registry)).await?;
    Ok(())
}

#[tokio::test]
async fn expiry_never_discards_an_armed_snapshot_for_an_active_extended_lease() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let provider = provider(root.path(), &registry).await?;
    let lease = LeaseId::new("extended");
    provider.lease(&request()?, &lease).await?;
    provider
        .sweep_retained_at(std::time::SystemTime::now() + std::time::Duration::from_secs(86400))
        .await?;
    provider.cancel(&lease).await?;
    provider.snapshot_to(&lease, &target(&registry)).await?;
    Ok(())
}

#[tokio::test]
async fn quota_refusal_and_purge_leave_no_snapshot_or_live_vm() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let provider = provider_config(
        root.path(),
        &registry,
        |config| config.released_snapshots.max_bytes = 1024,
        Arc::new(FakeLauncher::default()),
    )
    .await?;
    let lease = LeaseId::new("quota");
    assert!(provider.lease(&request()?, &lease).await.is_err());
    assert_eq!(provider.current_lease_count().await, 0);
    drop(provider);
    let provider = self::provider(root.path(), &registry).await?;
    provider.lease(&request()?, &lease).await?;
    provider.purge(&lease).await?;
    assert!(provider.snapshot_to(&lease, &target(&registry)).await.is_err());
    assert_eq!(registry.data.lock().await.pushes, 0);
    Ok(())
}

#[tokio::test]
async fn a_second_provider_cannot_create_dispatch_state_while_an_ordinary_lease_owns_it() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let first = provider(root.path(), &registry).await?;
    let ordinary = request_out(None)?;
    let lease = LeaseId::new("ordinary-owner");
    first.lease(&ordinary, &lease).await?;
    let state = first.config().state_dir.as_path();
    assert!(!state.join("dispatch.json").exists());
    let second = provider(root.path(), &registry).await?;
    let cleanup = trogon_sandbox_contract::ProviderCleanupRequest {
        identity: trogon_sandbox_contract::LeaseAttemptIdentity {
            lease_id: LeaseId::new("never-owned"),
            provider: ProviderKind::Firecracker,
            target: trogon_sandbox_contract::DispatchTarget {
                epoch: None,
                worker_id: trogon_sandbox_contract::WorkerId::new("foreign-worker")?,
                attempt_id: trogon_sandbox_contract::DispatchAttemptId::new("foreign-attempt")?,
            },
        },
        intent: CancelIntent::Purge,
    };
    assert!(matches!(
        second.cleanup_dispatch(&cleanup).await,
        trogon_sandbox_contract::ProviderCleanupResponse::Failed { .. }
    ));
    assert!(
        !state.join("dispatch.json").exists(),
        "cleanup must acquire state ownership before creating or fencing a dispatch ledger"
    );
    assert!(!state.join("dispatch.lock").exists());
    assert!(first.holds_lease(&lease).await);
    first.cancel(&lease).await?;
    Ok(())
}

#[tokio::test]
async fn restored_guest_clock_source_must_be_supported_before_a_lease_is_granted() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let launcher = Arc::new(FakeLauncher {
        exec_calls: Default::default(),
        clock_source: Some("tsc"),
        capture: None,
        tasks: Default::default(),
        launches: Default::default(),
    });
    let provider = provider_config(
        root.path(),
        &registry,
        |config| config.warm_pool.max_snapshots = 1,
        launcher,
    )
    .await?;
    let lease = LeaseId::new("unsupported-clock");
    let request = request_out(None)?;
    let result = provider.lease(&request, &lease).await;
    let error = result
        .err()
        .ok_or("an unsupported guest clock source must prevent granting a restorable lease")?;
    assert!(format!("{error:?}").contains("kvm-clock"));
    assert!(!provider.holds_lease(&lease).await);
    assert_eq!(provider.current_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn supported_clock_source_keeps_cold_seed_and_warm_restore_available() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let launcher = Arc::new(FakeLauncher::default());
    let provider = provider_config(
        root.path(),
        &registry,
        |config| config.warm_pool.max_snapshots = 1,
        launcher.clone(),
    )
    .await?;
    let request = request_out(None)?;
    for lease in [LeaseId::new("clock-seed"), LeaseId::new("clock-warm")] {
        provider.lease(&request, &lease).await?;
        assert!(provider.holds_lease(&lease).await);
        provider.cancel(&lease).await?;
    }
    let launches = launcher
        .launches
        .lock()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    assert!(!launches[0].1);
    assert!(
        launches[1].1,
        "the supported guest must actually restore the warm snapshot"
    );
    Ok(())
}

#[derive(Default)]
struct GatedCleanupLauncher {
    inner: FakeLauncher,
    entered: tokio::sync::Notify,
    proceed: tokio::sync::Notify,
    pid: std::sync::atomic::AtomicU32,
}

#[async_trait::async_trait]
impl VmLauncher for GatedCleanupLauncher {
    async fn launch(&self, dir: &VmDir) -> Result<VmProcess, crate::error::ProviderError> {
        let process = self.inner.launch(dir).await?;
        let pid = process
            .pid()
            .ok_or_else(|| crate::error::ProviderError::Launch("the process has no pid".into()))?;
        self.pid.store(pid, std::sync::atomic::Ordering::SeqCst);
        Ok(process)
    }

    async fn cleanup(&self, _dir: &VmDir) -> Result<(), crate::error::ProviderError> {
        self.entered.notify_one();
        self.proceed.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn capacity_counts_an_exited_vm_until_its_cleanup_releases_the_slot() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = Registry::start().await?;
    let launcher = Arc::new(GatedCleanupLauncher::default());
    let provider = provider_config(
        root.path(),
        &registry,
        |config| config.max_concurrent_leases = 1,
        launcher.clone(),
    )
    .await?;
    let request = request_out(None)?;
    let lease = LeaseId::new("cleanup-accounting");
    provider.lease(&request, &lease).await?;
    let pid = launcher.pid.load(std::sync::atomic::Ordering::SeqCst);
    assert!(pid > 1);
    assert!(
        tokio::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .await?
            .success()
    );
    tokio::time::timeout(Duration::from_secs(2), launcher.entered.notified()).await?;
    let reported = tokio::time::timeout(Duration::from_secs(2), provider.current_lease_count()).await?;
    let files_pending = provider.config().state_dir.as_path().join(lease.as_str()).exists();
    let replacement = provider
        .lease(&request, &LeaseId::new("replacement-during-cleanup"))
        .await;
    launcher.proceed.notify_one();
    tokio::time::timeout(Duration::from_secs(2), provider.cancel(&lease)).await??;
    assert!(files_pending, "the gate must hold actual directory teardown");
    let refusal = replacement.err().ok_or("the replacement lease must be refused")?;
    assert!(format!("{refusal:?}").contains("at capacity"));
    assert_eq!(
        reported, 1,
        "advertised capacity must include the slot still reserved by cleanup"
    );
    assert_eq!(provider.current_lease_count().await, 0);
    assert!(!provider.config().state_dir.as_path().join(lease.as_str()).exists());
    Ok(())
}

mod pipeline_tests;
