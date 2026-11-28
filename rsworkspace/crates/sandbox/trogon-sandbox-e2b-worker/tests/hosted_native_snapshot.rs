//! A native snapshot of a running E2B sandbox against a real account: heap and execution identity survive into an
//! independent fork, deletion is refused while the fork lives, and retirement completes once it is released.
//!
//! Skipped unless `SANDBOX_E2B_HOSTED_SNAPSHOT_TEST=1`; it reads `E2B_API_KEY` and optionally `SANDBOX_E2B_TEMPLATE`.
//! The worker runs in this process against a throwaway JetStream server, so `nats-server` must be installed.

mod hosted_support;
mod test_support;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use hosted_support::owned_lease::{OwnedLease, TENANT};
use hosted_support::{Boxed, TestResult};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    LeaseError, ListProcessesResponse, NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse, NativeSnapshotRef,
    ProcessState, RunCommandRequest, RunCommandResponse, SnapshotExportOutcome, TenantId, WorkerId,
};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals};
use trogon_sandbox_e2b_worker::{E2bWorkerError, Envd, run};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, tenant_scoped_kv_key};

const GATE: &str = "SANDBOX_E2B_HOSTED_SNAPSHOT_TEST";

const NODE_VERSION: &str = r#"process.stdout.write(process.versions.node + '\n');
const [major, minor] = process.versions.node.split('.').map(Number);
if (major < 22 || (major === 22 && minor < 7)) {
  process.stderr.write('Node 22.7+ is required for the hosted memory service\n');
  process.exit(126);
}
"#;

const SERVER: &str = r#"const http = require('node:http');
const crypto = require('node:crypto');
let counter = 41;
const nonce = crypto.randomBytes(16).toString('hex');
http.createServer((request, response) => {
  if (request.url === '/increment') counter += 1;
  else if (request.url !== '/state') {
    response.writeHead(404).end();
    return;
  }
  const body = JSON.stringify({counter, nonce});
  response.writeHead(200, {'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(body)});
  response.end(body);
}).listen(18765, '127.0.0.1');
"#;

const CLIENT: &str = r#"(async () => {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    let response;
    try {
      response = await fetch('http://127.0.0.1:18765/' + process.argv[1], {signal: AbortSignal.timeout(1000)});
    } catch (error) {
      if (attempt === 49) throw error;
      await new Promise(resolve => setTimeout(resolve, 100));
      continue;
    }
    if (!response.ok) throw new Error('guest memory request was refused');
    process.stdout.write(await response.text());
    return;
  }
})().catch(() => {
  process.stderr.write('guest memory request failed\n');
  process.exitCode = 1;
});
"#;

#[derive(Deserialize)]
struct Binding {
    template_id: String,
    build_id: String,
}

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    tenant: TenantId,
    reference: NativeSnapshotRef,
    binding: Binding,
    logical_name: String,
}

#[derive(Deserialize)]
struct MemoryState {
    counter: u32,
    nonce: String,
}

struct Served {
    cancel: CancellationToken,
    worker: tokio::task::JoinHandle<Result<(), E2bWorkerError>>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.worker.abort();
    }
}

struct Snapshot {
    nats: test_support::LocalNats,
    source: OwnedLease,
    fork: OwnedLease,
    logical_name: String,
    reference: Option<NativeSnapshotRef>,
    _served: Served,
}

fn native_image(reference: &NativeSnapshotRef) -> Result<Value, Boxed> {
    Ok(json!({"kind": "native_snapshot", "reference": serde_json::to_value(reference)?}))
}

impl Snapshot {
    async fn new() -> Result<Self, Boxed> {
        let source = OwnedLease::new(GATE, "hosted-snapshot-source")?;
        let fork = source.sharing("hosted-snapshot-fork");
        let nats = test_support::LocalNats::start().await;
        let envd = Envd::new(Duration::from_secs(10))?;
        let terminals = Arc::new(E2bTerminals::new(
            Arc::new(E2bGuestShell::new(source.provider.clone(), envd)),
            nats.client.clone(),
        ));
        let cancel = CancellationToken::new();
        let worker_id = WorkerId::new(format!("hosted-snapshot-{}", uuid::Uuid::new_v4()))?;
        let worker = tokio::spawn({
            let (client, provider, cancel) = (nats.client.clone(), source.provider.clone(), cancel.clone());
            async move {
                run(
                    client.clone(),
                    async_nats::jetstream::new(client),
                    provider,
                    &worker_id,
                    cancel,
                    terminals,
                    AuthorityConfig::new(AuthorityMode::Off),
                )
                .await
            }
        });
        let served = Served { cancel, worker };
        tokio::time::timeout(Duration::from_secs(15), source.provider.wait_ready())
            .await
            .map_err(|_| "the in-process worker did not become ready")?;
        Ok(Self {
            nats,
            source,
            fork,
            logical_name: format!("hosted-memory-{}", uuid::Uuid::new_v4()),
            reference: None,
            _served: served,
        })
    }

    async fn memory(&self, lease: &OwnedLease, operation: &str) -> Result<MemoryState, Boxed> {
        let sandbox = lease.sandbox().await?;
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id": lease.lease_id,
            "command": ["node", "-e", CLIENT, operation],
            "timeout_seconds": 10,
            "max_output_bytes": 1024,
        }))?;
        let response = lease.provider.exec().handle_run_command(&sandbox, &request).await;
        let RunCommandResponse::Completed(output) = response else {
            return Err(format!("the memory probe did not complete: {response:?}").into());
        };
        if output.exit_code != 0 || !output.timed_out.is_within_deadline() || !output.truncated.is_complete() {
            return Err("the memory probe must succeed without timeout or truncation".into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    async fn delete(&self, reference: &NativeSnapshotRef) -> Result<NativeSnapshotDeleteResponse, Boxed> {
        let request = NativeSnapshotDeleteRequest {
            tenant: TenantId::new(TENANT)?,
            reference: reference.clone(),
        };
        Ok(self.source.provider.native_snapshot_delete(&request).await)
    }

    async fn assert_restore_refused(&self, reference: &NativeSnapshotRef, label: &str) -> TestResult {
        let mut rejected = self.source.sharing(label);
        let restored = rejected.lease_from(300, native_image(reference)?).await;
        let refused = restored.is_err();
        if restored.is_ok() {
            rejected.cleanup().await?;
        }
        assert!(refused, "native cleanup intent must refuse a new restore at {label}");
        Ok(())
    }

    async fn exercise(&mut self) -> TestResult {
        let sandbox = self.source.lease(300).await?;
        let version: RunCommandRequest = serde_json::from_value(json!({
            "lease_id": self.source.lease_id,
            "command": ["node", "-e", NODE_VERSION],
            "timeout_seconds": 5,
            "max_output_bytes": 1024,
        }))?;
        match self.source.provider.exec().handle_run_command(&sandbox, &version).await {
            RunCommandResponse::Completed(output) if output.exit_code == 0 => {}
            other => return Err(format!("the guest must provide Node 22.7+ for the memory service: {other:?}").into()),
        }
        let launch: RunCommandRequest = serde_json::from_value(json!({
            "lease_id": self.source.lease_id,
            "command": ["node", "-e", SERVER],
            "detached": true,
            "deadline": "lease",
            "max_output_bytes": 1024,
        }))?;
        let RunCommandResponse::Started(started) =
            self.source.provider.exec().handle_run_command(&sandbox, &launch).await
        else {
            return Err("the memory server did not start".into());
        };
        let before = self.memory(&self.source, "increment").await?;
        assert_eq!(before.counter, 42, "the source heap must advance before capture");

        let captured = self
            .source
            .provider
            .snapshot_lease(&self.source.lease_id, &self.logical_name)
            .await;
        let SnapshotExportOutcome::Native(receipt) = captured else {
            return Err(format!("native capture failed: {captured:?}").into());
        };
        self.reference = Some(receipt.reference.clone());
        let kv = async_nats::jetstream::new(self.nats.client.clone())
            .get_key_value("sandbox-e2b-snapshot-executions")
            .await?;
        let data = kv
            .get(tenant_scoped_kv_key(
                &TenantId::new(TENANT)?,
                receipt.reference.id.as_str(),
            ))
            .await?
            .ok_or("the owned capture manifest is missing")?;
        let manifest: Manifest = serde_json::from_slice(&data)?;
        assert!(
            manifest.version == 3
                && manifest.tenant.as_str() == TENANT
                && manifest.reference == receipt.reference
                && manifest.logical_name == self.logical_name,
            "the manifest owner, reference and name must match this capture"
        );
        let build = uuid::Uuid::parse_str(&manifest.binding.build_id)?;
        assert!(
            !manifest.binding.template_id.is_empty() && !build.is_nil(),
            "the binding must pin a native template and a nonnil build"
        );
        assert_eq!(
            self.memory(&self.source, "increment").await?.counter,
            43,
            "the source must resume after the memory capture"
        );

        let fork_sandbox = self.fork.lease_from(300, native_image(&receipt.reference)?).await?;
        assert_ne!(
            self.source.sandbox_id, self.fork.sandbox_id,
            "the fork must be an independent sandbox"
        );
        let processes = self
            .fork
            .provider
            .exec()
            .handle_list_processes(&fork_sandbox, self.fork.lease_id.as_str())
            .await;
        assert!(
            matches!(&processes, ListProcessesResponse::Processes { processes }
                if processes.len() == 1
                    && processes[0].execution_id == started.execution_id
                    && processes[0].state == ProcessState::Running),
            "the fork must adopt the original running execution id: {processes:?}"
        );
        let fork = self.memory(&self.fork, "increment").await?;
        assert!(
            fork.counter == 43 && fork.nonce == before.nonce,
            "the fork must retain live heap state rather than relaunch the server"
        );
        assert!(
            self.memory(&self.source, "increment").await?.counter == 44
                && self.memory(&self.fork, "state").await?.counter == 43,
            "the source and fork heaps must change independently"
        );
        self.source.cleanup().await?;
        let surviving = self.memory(&self.fork, "increment").await?;
        assert!(
            surviving.counter == 44 && surviving.nonce == before.nonce,
            "the adopted live heap must survive deleting the source"
        );

        let refused = self.delete(&receipt.reference).await?;
        assert!(
            matches!(&refused, NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { message })
                if message.contains("running sandboxes using it")),
            "an active fork must produce the provider refusal: {refused:?}"
        );
        let custody = async_nats::jetstream::new(self.nats.client.clone())
            .get_key_value("sandbox-e2b-snapshot-custody")
            .await?;
        let pending = custody
            .get(tenant_scoped_kv_key(
                &TenantId::new(TENANT)?,
                receipt.reference.id.as_str(),
            ))
            .await?
            .ok_or("a refused deletion lost its durable custody")?;
        let pending: Value = serde_json::from_slice(&pending)?;
        assert_eq!(
            pending["phase"], "cleanup",
            "a provider refusal must retain the cleanup intent"
        );
        assert_eq!(
            self.memory(&self.fork, "state").await?.counter,
            44,
            "a refused retirement must preserve the existing fork"
        );
        self.assert_restore_refused(&receipt.reference, "pending-cleanup")
            .await?;

        self.fork.cleanup().await?;
        self.settle_retirement(&receipt.reference).await?;
        assert_eq!(
            self.delete(&receipt.reference).await?,
            NativeSnapshotDeleteResponse::NotFound,
            "a repeated deletion must report not found"
        );
        self.assert_restore_refused(&receipt.reference, "retired").await?;
        Ok(())
    }

    /// Repeats the deletion until the provider has retired the snapshot, tolerating the release still settling.
    async fn settle_retirement(&self, reference: &NativeSnapshotRef) -> TestResult {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match self.delete(reference).await? {
                    NativeSnapshotDeleteResponse::Deleted | NativeSnapshotDeleteResponse::NotFound => {
                        break Ok::<(), Boxed>(());
                    }
                    NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { message })
                        if message.contains("in progress") || message.contains("running sandboxes using it") =>
                    {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    other => return Err(format!("the released fork did not permit retirement: {other:?}").into()),
                }
            }
        })
        .await
        .map_err(|_| "native retirement did not settle after the fork was released")?
    }

    async fn cleanup(&self) -> TestResult {
        let mut failures = Vec::new();
        for lease in [&self.fork, &self.source] {
            if let Err(error) = lease.cleanup().await {
                failures.push(format!("owned sandbox cleanup: {error}"));
            }
        }
        if let Some(reference) = &self.reference
            && let Err(error) = self.settle_retirement(reference).await
        {
            failures.push(format!("native snapshot retirement: {error}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; ").into())
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "paid hosted E2B persistent snapshot: set SANDBOX_E2B_HOSTED_SNAPSHOT_TEST=1 with E2B_API_KEY and nats-server installed; creates one snapshot, a source and a fork with a 300s TTL"]
async fn hosted_native_snapshot_preserves_heap_execution_and_pinned_build() -> TestResult {
    let mut probe = Snapshot::new().await?;
    let outcome = AssertUnwindSafe(probe.exercise()).catch_unwind().await;
    let cleanup = probe.cleanup().await;
    match outcome {
        Ok(Ok(())) => cleanup,
        Ok(Err(error)) => match cleanup {
            Ok(()) => Err(format!("the hosted native snapshot failed; owned cleanup confirmed: {error}").into()),
            Err(cleanup) => {
                Err(format!("the hosted native snapshot failed: {error}; cleanup also failed: {cleanup}").into())
            }
        },
        Err(panic) => {
            if let Err(error) = cleanup {
                eprintln!("hosted native cleanup after a panic failed: {error}");
            }
            std::panic::resume_unwind(panic)
        }
    }
}
