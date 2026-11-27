//! The public `denied_connect_fails` conformance check against the actual worker binary: an invalid probe endpoint
//! fails before any effect, and a Python image without `nc` still earns the check against an owned listener that
//! witnesses independent connections before and after.

#![cfg(unix)]

mod egress_probe_support;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::process::Command;
use trogon_sandbox_conformance::{CheckFailureError, ConformanceGrant, GrantLedger, NativeCleanupAuthority, Target};
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_providers::{DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER, DOCKER_LABEL_ROLE};

use egress_probe_support::{Process, WAIT, lock};

const EGRESS_IMAGE: &str = "sandbox-egress-proxyd:dev";
const LISTENER: &str = r#"import json,socket,sys
nonce=sys.argv[1].encode()
listener=socket.socket(socket.AF_INET,socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
listener.bind(('0.0.0.0',0))
listener.listen(8)
print(json.dumps({'port':listener.getsockname()[1]}),flush=True)
accepted=matched=0
while True:
 connection,_=listener.accept()
 connection.settimeout(3)
 accepted+=1
 try:
  data=b''
  while len(data)<len(nonce):
   chunk=connection.recv(len(nonce)-len(data))
   if not chunk: break
   data+=chunk
  if data==nonce:
   matched+=1
   connection.sendall(nonce)
 except (TimeoutError,OSError):
  pass
 finally:
  connection.close()
 print(json.dumps({'accepted':accepted,'matched':matched}),flush=True)
"#;

const POSITIVE: &str = r#"import socket,sys
address=sys.argv[1]
port=int(sys.argv[2])
nonce=sys.argv[3].encode()
connection=socket.socket(socket.AF_INET,socket.SOCK_STREAM)
connection.settimeout(3)
try:
 connection.connect((address,port))
 connection.sendall(nonce)
 data=b''
 while len(data)<len(nonce):
  chunk=connection.recv(len(nonce)-len(data))
  if not chunk: break
  data+=chunk
 assert data==nonce,'the exact owned listener did not return its nonce'
 print('CONNECTED')
finally:
 connection.close()
"#;

fn hash(path: &Path) -> anyhow::Result<String> {
    let mut source = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Digests of the sources this lane depends on, so the receipt shows they did not change while it ran.
fn source_hashes(root: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let paths = [
        "crates/sandbox/trogon-sandbox-conformance/src/lib.rs",
        "crates/sandbox/trogon-sandbox-conformance/src/egress_checks.rs",
        "crates/sandbox/trogon-sandbox-docker-worker/src/main.rs",
        "crates/sandbox/trogon-sandbox-docker-worker/src/worker.rs",
        "crates/sandbox/trogon-sandbox-docker-worker/src/adapter.rs",
        "crates/sandbox/trogon-sandbox-docker-worker/src/adapter/assigned.rs",
        "crates/sandbox/trogon-sandbox-docker-worker/tests/conformance_egress_probe.rs",
        "crates/sandbox/trogon-sandbox-docker-worker/tests/egress_probe_support/mod.rs",
        "crates/sandbox/trogon-sandbox-providers/src/local_docker_egress.rs",
        "crates/sandbox/trogon-sandbox-providers/src/docker_engine.rs",
    ];
    paths
        .into_iter()
        .map(|path| Ok((path.into(), hash(&root.join(path))?)))
        .collect()
}

async fn listener_logs(id: &str) -> anyhow::Result<Vec<Value>> {
    let logs = egress_probe_support::successful(Command::new("docker").args(["logs", id])).await?;
    std::str::from_utf8(&logs)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

async fn positive(id: &str, endpoint: SocketAddr, nonce: &str) -> anyhow::Result<()> {
    let output = egress_probe_support::successful(Command::new("docker").args([
        "exec",
        id,
        "python3",
        "-c",
        POSITIVE,
        &endpoint.ip().to_string(),
        &endpoint.port().to_string(),
        nonce,
    ]))
    .await?;
    ensure!(
        output == b"CONNECTED\n",
        "independent unrestricted Python connection did not prove the exact listener"
    );
    Ok(())
}

fn denied_connect_check() -> anyhow::Result<trogon_sandbox_conformance::CheckFn> {
    Ok(trogon_sandbox_conformance::checks()
        .into_iter()
        .find(|(name, _)| *name == "denied_connect_fails")
        .context("public egress check is missing")?
        .1)
}

#[tokio::test]
async fn invalid_egress_endpoints_are_refused_before_any_dispatch() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut children = Vec::new();
    let result = async {
        let (client, _) = egress_probe_support::nats(&mut children, directory.path()).await?;
        let mut effects = client.subscribe("sandbox.>").await?;
        client.flush().await?;
        let ledger = GrantLedger::default();
        let mut target = Target::new(client.clone(), ProviderKind::LocalDocker).with_grant_ledger(ledger.clone());
        let check = denied_connect_check()?;
        for endpoint in ["0.0.0.0:443", "[::]:443", "127.0.0.1:0"] {
            target.egress_probe_address = endpoint.parse()?;
            let outcome = tokio::time::timeout(Duration::from_secs(1), check(&target)).await?;
            ensure!(
                matches!(&outcome, Err(CheckFailureError::Mismatch { expected, .. })
                    if expected == "a concrete egress probe endpoint"),
                "invalid endpoint {endpoint} did not fail before effects: {outcome:?}"
            );
        }
        ensure!(ledger.snapshot().is_empty(), "invalid endpoint acquired a native grant");
        ensure!(
            tokio::time::timeout(Duration::from_millis(100), effects.next())
                .await
                .is_err(),
            "invalid endpoint published a provider request"
        );
        anyhow::Ok(())
    }
    .await;
    for child in children.iter_mut().rev() {
        child.stop().await?;
    }
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "native public egress oracle: SANDBOX_DOCKER_EGRESS_PROBE_TEST=1, existing cached SANDBOX_DOCKER_PYTHON_NO_NC_IMAGE, cached egress image, Docker and nats-server; run with --ignored"]
async fn python_image_without_nc_earns_the_public_denied_connect_check() -> anyhow::Result<()> {
    let env = trogon_std::env::SystemEnv;
    ensure!(
        trogon_std::env::ReadEnv::var(&env, "SANDBOX_DOCKER_EGRESS_PROBE_TEST").as_deref() == Ok("1"),
        "explicit native egress test opt-in required"
    );
    let image_reference = trogon_std::env::ReadEnv::var(&env, "SANDBOX_DOCKER_PYTHON_NO_NC_IMAGE")
        .context("select an existing cached Python3 image without nc; this test never builds or pulls")?;
    let docker = Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(5));
    let cached_image = docker
        .inspect_image(&image_reference)
        .await
        .context("configured Python3/no-nc image must already be cached")?;
    ensure!(
        cached_image
            .config
            .as_ref()
            .and_then(|config| config.volumes.as_ref())
            .is_none_or(|volumes| volumes.is_empty()),
        "the explicit Python fixture image must not declare anonymous volumes"
    );
    let image = cached_image
        .id
        .context("cached Python image lacks immutable identity")?;
    ensure!(
        image
            .strip_prefix("sha256:")
            .is_some_and(egress_probe_support::canonical),
        "Python image is not canonical"
    );
    let egress = docker
        .inspect_image(EGRESS_IMAGE)
        .await
        .context("existing egress image must already be cached")?
        .id
        .context("cached egress image lacks immutable identity")?;
    ensure!(
        egress
            .strip_prefix("sha256:")
            .is_some_and(egress_probe_support::canonical),
        "egress image is not canonical"
    );
    let directory = tempfile::tempdir()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .context("workspace root missing")?;
    let sources = source_hashes(root)?;
    let worker_binary = PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-docker-worker"));
    let worker_hash = hash(&worker_binary)?;
    let owner = format!("egress-worker-{}", uuid::Uuid::now_v7());
    let listener_owner = format!("egress-listener-{}", uuid::Uuid::now_v7());
    let nonce = uuid::Uuid::now_v7().simple().to_string();
    let signer = egress_probe_support::signer()?;
    let ledger = GrantLedger::default();
    let mut children = Vec::new();
    let mut connection = None;
    let mut worker_id = None;
    let mut listener_id = None;
    let mut control_grant: Option<ConformanceGrant> = None;
    let (trace, stop_trace, trace_task) = egress_probe_support::observe(&docker, owner.clone(), image.clone());
    let mut report = json!({"phase":"preflight","primary_outcome":"unreached","fallback_used":false,
        "image":{"reference":image_reference,"id":image},"egress_image":{"reference":EGRESS_IMAGE,"id":egress},
        "source_sha256":sources,"worker_sha256":worker_hash,"driver_sha256":hash(&std::env::current_exe()?)?,
        "source_binary_binding":"parent_build_receipt_required"});
    let operation = std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(180), async {
            let (client, nats_url) = egress_probe_support::nats(&mut children, directory.path()).await?;
            connection = Some(client.clone());
            let mut command = Command::new(&worker_binary);
            egress_probe_support::inherit_runtime(&mut command);
            command
                .env("SANDBOX_NATS_URL", &nats_url)
                .env("SANDBOX_DOCKER_WORKER_OWNER", &owner)
                .env("SANDBOX_DOCKER_STATE_DIR", directory.path().join("worker-state"))
                .env("SANDBOX_IMAGE", &image)
                .env("SANDBOX_EGRESS_PROXY_IMAGE", &egress)
                .env("SANDBOX_WARM_POOL_ENABLED", "false")
                .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
                .env("SANDBOX_DISPATCH_VERIFY", "require")
                .env("SANDBOX_FENCE_EPOCH", "require")
                .env("SANDBOX_DISPATCH_FENCE_FILE", directory.path().join("worker.fence"))
                .env(
                    "SANDBOX_DISPATCH_PUBLIC_KEYS",
                    format!("{}:{}", signer.key_id(), signer.public_key_b64()),
                );
            children.push(Process::start("docker-worker", &mut command)?);
            let heartbeat = egress_probe_support::heartbeat(
                &client,
                children.last_mut().context("owned worker child missing")?,
            )
            .await?;
            worker_id = Some(heartbeat.worker_id.clone());
            report["actual_worker"] = serde_json::to_value(&heartbeat)?;
            report["phase"] = "owned_listener".into();
            let created = egress_probe_support::successful(Command::new("docker").args([
                "run",
                "--detach",
                "--rm",
                "--pull",
                "never",
                "--network",
                "bridge",
                "--read-only",
                "--cap-drop",
                "ALL",
                "--pids-limit",
                "64",
                "--memory",
                "64m",
                "--label",
                &format!("{DOCKER_LABEL_OWNER}={listener_owner}"),
                "--label",
                &format!("{DOCKER_LABEL_ROLE}=egress-listener"),
                "--entrypoint",
                "python3",
                &image,
                "-u",
                "-c",
                LISTENER,
                &nonce,
            ]))
            .await?;
            let id = std::str::from_utf8(&created)?.trim().to_owned();
            ensure!(
                egress_probe_support::canonical(&id),
                "listener create did not retain canonical native identity"
            );
            listener_id = Some(id.clone());
            let endpoint = tokio::time::timeout(WAIT, async {
                loop {
                    let found = docker.inspect_container(&id, None).await?;
                    ensure!(
                        found.id.as_deref() == Some(id.as_str())
                            && found.image.as_deref() == Some(image.as_str())
                            && found.state.as_ref().and_then(|state| state.running) == Some(true)
                            && found
                                .config
                                .as_ref()
                                .and_then(|config| config.labels.as_ref())
                                .is_some_and(|labels| labels.get(DOCKER_LABEL_OWNER) == Some(&listener_owner)),
                        "listener must be exact running owned cached image"
                    );
                    let ip = found
                        .network_settings
                        .as_ref()
                        .and_then(|network| network.networks.as_ref())
                        .and_then(|networks| networks.get("bridge"))
                        .and_then(|network| network.ip_address.as_ref())
                        .context("owned listener bridge IP missing")?
                        .parse::<IpAddr>()?;
                    ensure!(
                        ip.is_ipv4() && !ip.is_unspecified() && !ip.is_loopback(),
                        "listener must use its actual raw bridge IPv4"
                    );
                    let logs = listener_logs(&id).await?;
                    if let Some(port) = logs
                        .first()
                        .and_then(|line| line["port"].as_u64())
                        .and_then(|port| u16::try_from(port).ok())
                        .filter(|port| *port > 0)
                    {
                        return anyhow::Ok(SocketAddr::new(ip, port));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .context("owned listener did not become ready")??;
            report["listener"] = json!({"container_id":id,"endpoint":endpoint});
            let mut target = Target::new(client.clone(), ProviderKind::LocalDocker)
                .for_worker(&heartbeat)?
                .with_grant_ledger(ledger.clone());
            target.signer = Some(Arc::clone(&signer));
            target.image = image.clone();
            target.egress_probe_address = endpoint;
            target.denied_connect_fails = heartbeat.profile.isolation.egress.deny_all.is_supported();
            target.spec = Some(egress_probe_support::suite_spec(&image)?);
            let control = egress_probe_support::acquire_control(&client, &signer, &heartbeat, &image).await?;
            control_grant = Some(control.clone());
            let control_id = control.native.sandbox_id.as_str();
            let control_native = docker.inspect_container(control_id, None).await?;
            ensure!(
                control.native_cleanup_authority == NativeCleanupAuthority::RequestMatched
                    && control_native.id.as_deref() == Some(control_id)
                    && control_native.image.as_deref() == Some(image.as_str())
                    && control_native
                        .host_config
                        .as_ref()
                        .and_then(|host| host.network_mode.as_deref())
                        == Some("bridge")
                    && control_native
                        .config
                        .as_ref()
                        .and_then(|config| config.labels.as_ref())
                        .is_some_and(|labels| labels.get(DOCKER_LABEL_OWNER) == Some(&owner)
                            && labels.get(DOCKER_LABEL_LEASE_ID).map(String::as_str)
                                == Some(control.lease_id.as_str())),
                "independent AllowAll control must be exact assigned native owner/image/bridge"
            );
            let tools = egress_probe_support::successful(Command::new("docker").args([
                "exec",
                control_id,
                "sh",
                "-c",
                "command -v python3 >/dev/null 2>&1 && ! command -v nc >/dev/null 2>&1 && python3 -c 'import socket,sys; assert sys.version_info.major == 3; print(\"PYTHON3_NO_NC\")'",
            ]))
            .await?;
            ensure!(
                tools == b"PYTHON3_NO_NC\n",
                "the configured real image must positively provide Python3 and lack nc"
            );
            report["python3_present_nc_absent"] = true.into();
            positive(control_id, endpoint, &nonce).await?;
            report["independent_before_connected"] = true.into();
            report["phase"] = "actual_public_denied_connect_check".into();
            let outcome = denied_connect_check()?(&target).await;
            report["primary_outcome"] = json!({"accepted":outcome.is_ok(),
                "error":outcome.as_ref().err().map(ToString::to_string)});
            positive(control_id, endpoint, &nonce).await?;
            report["independent_after_connected"] = true.into();
            let counts = listener_logs(&id).await?;
            let observed = counts.last().context("listener has no observed connection receipt")?;
            ensure!(
                observed["matched"].as_u64().is_some_and(|count| count >= 2),
                "owned listener must witness both real independent Python connections"
            );
            report["listener_connections"] = observed.clone();
            outcome?;
            report["phase"] = "public_check_accepted".into();
            anyhow::Ok(())
        })
        .await
        .context("native egress body timed out; retain exact allocation receipts")?
    })
    .catch_unwind()
    .await;
    report["primary_phase"] = report["phase"].clone();
    if let Ok(Err(error)) = &operation {
        report["operation_error"] = format!("{error:#}").chars().take(4096).collect::<String>().into();
    }
    eprintln!(
        "native egress primary receipt: {}",
        json!({"phase":report["primary_phase"],"outcome":report["primary_outcome"],
            "operation_error":report["operation_error"]})
    );
    let grants: Vec<ConformanceGrant> = control_grant.into_iter().chain(ledger.snapshot()).collect();
    report["grants"] = json!(
        grants
            .iter()
            .map(
                |grant| json!({"provider":grant.provider,"lease_id":grant.lease_id,"native":grant.native,
                "native_cleanup_authority":grant.native_cleanup_authority})
            )
            .collect::<Vec<_>>()
    );
    let mut cleanup_errors = Vec::new();
    let mut exact_absent = BTreeSet::new();
    for grant in &grants {
        let sandbox_id = grant.native.sandbox_id.as_str();
        if let (Some(client), Some(worker)) = (&connection, &worker_id) {
            if egress_probe_support::cancel(client, &signer, grant, worker)
                .await
                .is_err()
            {
                cleanup_errors.push("exact_assigned_cancel_failed");
            }
        } else {
            cleanup_errors.push("native_grant_without_worker_authority");
        }
        if egress_probe_support::wait_absent(&docker, sandbox_id).await.is_ok() {
            exact_absent.insert(sandbox_id.to_owned());
        } else {
            cleanup_errors.push("product_exact_native_absence_unconfirmed");
        }
    }
    let mut receipts = Vec::new();
    for child in children.iter_mut().rev() {
        match child.stop().await {
            Ok(receipt) => {
                if receipt["forced_stop"] == true || receipt["output_overflow"] == true || receipt["exit"] != 0 {
                    cleanup_errors.push("owned_child_teardown_not_clean");
                }
                receipts.push(receipt);
            }
            Err(_) => cleanup_errors.push("owned_child_teardown_failed"),
        }
    }
    report["children"] = json!(receipts);
    stop_trace.cancel();
    let mut trace_task = trace_task;
    if !matches!(tokio::time::timeout(WAIT, &mut trace_task).await, Ok(Ok(()))) {
        trace_task.abort();
        let _ = trace_task.await;
        cleanup_errors.push("native_observer_teardown_failed");
    }
    let trace = lock(&trace).clone();
    report["native_allocations"] = serde_json::to_value(&trace)?;
    if !trace.errors.is_empty() {
        cleanup_errors.push("native_allocation_provenance_incomplete");
    }
    for grant in &grants {
        if !trace
            .allocations
            .get(grant.native.sandbox_id.as_str())
            .is_some_and(|allocation| {
                allocation.lease == grant.lease_id.as_str()
                    && allocation.role == "workload"
                    && allocation.image_verified
                    && matches!(allocation.network.as_str(), "none" | "bridge")
            })
        {
            cleanup_errors.push("exact_grant_owner_image_network_provenance_unconfirmed");
        }
    }
    report["native_denied_network_observed"] = trace
        .allocations
        .values()
        .any(|allocation| allocation.network == "none")
        .into();
    let remaining = match egress_probe_support::owned(&docker, &owner).await {
        Ok(ids) => ids,
        Err(_) => {
            cleanup_errors.push("owned_native_inventory_unknown");
            Vec::new()
        }
    };
    report["remaining_worker_ids_before_fallback"] = json!(remaining);
    for id in remaining {
        report["fallback_used"] = true.into();
        cleanup_errors.push("product_native_cleanup_required_fallback");
        if let Some(allocation) = trace.allocations.get(&id) {
            let _ = egress_probe_support::remove_owned(&docker, &id, &owner, Some(&allocation.lease)).await;
        }
        if !egress_probe_support::absent(&docker, &id).await.unwrap_or(false) {
            cleanup_errors.push("remaining_exact_native_absence_unknown");
        }
    }
    if !matches!(egress_probe_support::owned(&docker, &owner).await, Ok(ids) if ids.is_empty()) {
        cleanup_errors.push("owned_native_inventory_not_empty_or_unknown");
    }
    if let Some(id) = listener_id {
        let stopped = matches!(
            egress_probe_support::output(Command::new("docker").args(["stop", "--time", "5", &id])).await,
            Ok(result) if result.status.success()
        );
        if !stopped || egress_probe_support::wait_absent(&docker, &id).await.is_err() {
            report["fallback_used"] = true.into();
            cleanup_errors.push("listener_cleanup_required_fallback");
            let _ = egress_probe_support::remove_owned(&docker, &id, &listener_owner, None).await;
        }
        if !egress_probe_support::absent(&docker, &id).await.unwrap_or(false) {
            cleanup_errors.push("exact_listener_absence_unknown");
        }
    }
    match egress_probe_support::owned(&docker, &listener_owner).await {
        Ok(ids) => {
            report["remaining_listener_ids_before_fallback"] = json!(ids);
            for id in ids {
                report["fallback_used"] = true.into();
                cleanup_errors.push("owned_listener_discovery_required_fallback");
                let _ = egress_probe_support::remove_owned(&docker, &id, &listener_owner, None).await;
                if !egress_probe_support::absent(&docker, &id).await.unwrap_or(false) {
                    cleanup_errors.push("discovered_exact_listener_absence_unknown");
                }
            }
        }
        Err(_) => cleanup_errors.push("owned_listener_inventory_unknown"),
    }
    report["exact_native_ids_absent"] = json!(exact_absent);
    report["source_stable"] = (source_hashes(root).ok().as_ref() == Some(&sources)).into();
    report["worker_binary_stable"] = (hash(&worker_binary).ok().as_ref() == Some(&worker_hash)).into();
    report["cached_images_preserved"] =
        (docker.inspect_image(&image).await.is_ok() && docker.inspect_image(&egress).await.is_ok()).into();
    report["cleanup_errors"] = json!(cleanup_errors);
    eprintln!("native public egress receipt: {report}");
    ensure!(
        cleanup_errors.is_empty()
            && report["source_stable"] == true
            && report["worker_binary_stable"] == true
            && report["cached_images_preserved"] == true
            && report["native_denied_network_observed"] == true,
        "native public egress fixture/provenance failed; primary={}; cleanup={cleanup_errors:?}",
        report["primary_outcome"]
    );
    match operation {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
