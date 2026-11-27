// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_nats::jetstream;
use base64::Engine;
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    ExecutionId, LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse,
    WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{DispatchSigner, EpochMode, VerbScope, subjects, tenant_scoped_kv_key};

use crate::authority_support;
use crate::ensure_support::{Boxed, Context, check};
use crate::native_access_support::NativeAccess;
use crate::native_command_support::{Binding, Guest, NativeEnvd};
use crate::native_process_support::{Api, SANDBOX_ID};
use crate::worker_process_support::{self, Nats, Process, signed};

pub const OLD_KEY: &str = "fence-original-strong-launch";
pub const FRESH_KEY: &str = "fence-fresh-strong-launch";
const OLD_EFFECT: &str = "fence-original-effect";
const FRESH_EFFECT: &str = "fence-fresh-effect";

pub struct Assigned {
    pub lease_id: LeaseId,
    pub target: DispatchTarget,
    pub scope: VerbScope,
}

pub struct Fixture {
    pub nats: Nats,
    pub guest: Arc<Guest>,
    pub api: Api,
    pub ledger: PathBuf,
    signer: DispatchSigner,
    worker: Process,
    heartbeats: async_nats::Subscriber,
    native: NativeAccess,
    envd: NativeEnvd,
    _directory: tempfile::TempDir,
}

pub struct Fresh {
    pub refused: bool,
    pub bootstraps_before: usize,
    pub bootstraps_after: usize,
    pub effect: bool,
}

pub struct Cleanup {
    pub product: bool,
    pub native_created: bool,
}

pub struct OriginalAuthority {
    assignment: Vec<u8>,
    command: Value,
    pub fence: Vec<u8>,
}

impl Fixture {
    pub async fn start(unopenable: bool) -> Result<Self, Boxed> {
        let nats = Nats::start().await;
        jetstream::new(nats.client.clone()).query_account().await?;
        let guest = Guest::create()?;
        let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
        let api = Api::start(Arc::clone(&guest)).await?;
        let native = NativeAccess::start(envd.url.clone(), Arc::clone(&api.control)).await?;
        let directory = tempfile::tempdir()?;
        let ledger = directory.path().join("dispatch.sqlite");
        if unopenable {
            std::fs::create_dir(ledger.with_extension("fence"))?;
        }
        let signer = DispatchSigner::new("native-fence-fixture", [101; 32]);
        let heartbeats = nats
            .client
            .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
            .await?;
        nats.client.flush().await?;
        let worker = worker_process_support::worker_with_epoch_mode(
            &nats.url,
            &api.url,
            &native.url,
            &ledger,
            &signer,
            10,
            EpochMode::Require,
        )?;
        Ok(Self {
            nats,
            guest,
            api,
            ledger,
            signer,
            worker,
            heartbeats,
            native,
            envd,
            _directory: directory,
        })
    }

    pub fn fence_path(&self) -> PathBuf {
        self.ledger.with_extension("fence")
    }

    pub async fn readiness(&mut self) -> Result<WorkerHeartbeat, Boxed> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                check(self.worker.is_alive()?, "the worker exited before establishing readiness")?;
                tokio::select! {
                    message = self.heartbeats.next() => {
                        let message = message.context("owned heartbeat subscription closed")?;
                        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
                        assert!(heartbeat.supports_lease_epoch.is_supported() && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1,
                            "the durability test must observe the actual assigned epoch transport");
                        return Ok::<_, Boxed>(heartbeat);
                    }
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
            }
        })
        .await
        .context("actual worker neither established readiness nor refused startup")?
    }

    pub async fn admit(&self, heartbeat: &WorkerHeartbeat) -> Result<Assigned, Boxed> {
        let target = DispatchTarget {
            worker_id: heartbeat.worker_id.clone(),
            attempt_id: DispatchAttemptId::new("native-fence-attempt")?,
            epoch: Some(LeaseEpoch::new(7)?),
        };
        let lease_id = LeaseId::new("native-fence-lease");
        let mut request = authority_support::lease_request()?;
        request.ttl_seconds = 600;
        let scope = VerbScope {
            tenant: request.tenant.clone(),
            lease_id: lease_id.clone(),
            worker_id: Some(target.worker_id.clone()),
            attempt_id: Some(target.attempt_id.clone()),
            epoch: target.epoch,
        };
        let dispatch = ProviderDispatchPayload {
            lease_id: lease_id.clone(),
            request,
            correlation_id: None,
            resolved_drives: vec![],
            target: Some(target.clone()),
        };
        let value: Value =
            worker_process_support::dispatch(&self.nats, &self.signer, &dispatch, &target.worker_id).await?;
        let grant: ProviderLease = serde_json::from_value(value)?;
        check(
            grant.sandbox_id.as_str() == SANDBOX_ID
                && grant.worker_id.as_ref() == Some(&target.worker_id)
                && grant.attempt_id.as_ref() == Some(&target.attempt_id)
                && grant.epoch == target.epoch,
            "durability test requires an exact signed native grant",
        )?;
        Ok(Assigned {
            lease_id,
            target,
            scope,
        })
    }

    async fn run(&self, assigned: &Assigned, request: &RunCommandRequest) -> Result<Value, Boxed> {
        signed(
            &self.nats,
            &self.signer,
            subjects::provider_run_command(ProviderKind::E2b, &assigned.lease_id),
            request,
            &assigned.scope,
        )
        .await
    }

    pub async fn ordinary(&self, assigned: &Assigned) -> Result<(), Boxed> {
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":assigned.lease_id,"command":["printf","native-fence-ordinary"],"run_as_user":"root",
        }))?;
        let before = self.guest.bootstrap_calls.load(Ordering::SeqCst);
        let response: RunCommandResponse = serde_json::from_value(self.run(assigned, &request).await?)?;
        assert!(
            matches!(response, RunCommandResponse::Completed(ref output) if output.exit_code == 0 && output.timed_out.is_within_deadline())
                && self.guest.bootstrap_calls.load(Ordering::SeqCst) == before + 1,
            "ready fixture must prove actual ordinary native command delivery"
        );
        Ok(())
    }

    pub async fn sentinel(&self, assigned: &Assigned) -> Result<Binding, Boxed> {
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":assigned.lease_id,"command":["sleep","300"],"run_as_user":"root","detached":true,"deadline":"lease",
        }))?;
        let response: RunCommandResponse = serde_json::from_value(self.run(assigned, &request).await?)?;
        assert!(
            matches!(response, RunCommandResponse::Started(_)),
            "the independent sentinel must start"
        );
        self.guest.last_binding()
    }

    fn strong(&self, assigned: &Assigned, key: &str, effect: &str) -> Result<RunCommandRequest, Boxed> {
        let command = format!("printf owned >{}; sleep 300 & wait", self.guest.path(effect).display());
        Ok(serde_json::from_value(json!({
            "lease_id":assigned.lease_id,"launch_key":key,
            "require_launch_cancellation":true,"command":["bash","-c",command],"run_as_user":"root",
            "env":{},"detached":true,"deadline":"lease"}))?)
    }

    pub async fn original(&self, assigned: &Assigned) -> Result<(ExecutionId, Binding), Boxed> {
        let request = self.strong(assigned, OLD_KEY, OLD_EFFECT)?;
        let response: RunCommandResponse = serde_json::from_value(self.run(assigned, &request).await?)?;
        let RunCommandResponse::Started(started) = response else {
            return Err("intact durable authority must start the positive strong launch".into());
        };
        self.wait_effect(OLD_EFFECT).await?;
        Ok((started.execution_id, self.guest.last_binding()?))
    }

    async fn wait_effect(&self, name: &str) -> Result<(), Boxed> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.guest.effect(name)?.is_some() {
                    return Ok::<_, Boxed>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("the actual strong native positive effect was not observed")??;
        Ok(())
    }

    pub async fn fresh(&self, assigned: &Assigned) -> Result<Fresh, Boxed> {
        let before = self.guest.bootstrap_calls.load(Ordering::SeqCst);
        let request = self.strong(assigned, FRESH_KEY, FRESH_EFFECT)?;
        let value = self.run(assigned, &request).await?;
        let refused = if value["error"] == "dispatch_auth" {
            true
        } else {
            match serde_json::from_value::<RunCommandResponse>(value)? {
                RunCommandResponse::Denied(_) => true,
                RunCommandResponse::Started(_) => {
                    self.wait_effect(FRESH_EFFECT).await?;
                    false
                }
                RunCommandResponse::Completed(_) => false,
            }
        };
        Ok(Fresh {
            refused,
            bootstraps_before: before,
            bootstraps_after: self.guest.bootstrap_calls.load(Ordering::SeqCst),
            effect: self.guest.effect(FRESH_EFFECT)?.is_some(),
        })
    }

    pub async fn cancel_original(&self, assigned: &Assigned, execution: &ExecutionId) -> Result<bool, Boxed> {
        let request = CancelCommandLaunchRequest {
            lease_id: assigned.lease_id.clone(),
            launch_key: LaunchKey::new(OLD_KEY)?,
        };
        let subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &assigned.lease_id);
        let first: CancelCommandLaunchResponse =
            signed(&self.nats, &self.signer, subject.clone(), &request, &assigned.scope).await?;
        let replay: CancelCommandLaunchResponse =
            signed(&self.nats, &self.signer, subject, &request, &assigned.scope).await?;
        Ok(first
            == CancelCommandLaunchResponse::Canceled {
                execution_id: Some(execution.clone()),
            }
            && first == replay)
    }

    async fn authority_values(&self, assigned: &Assigned) -> Result<(Vec<u8>, Value), Boxed> {
        let context = jetstream::new(self.nats.client.clone());
        let assignments = context.get_key_value("sandbox-e2b-assignments").await?;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&(&assigned.target.worker_id, &assigned.lease_id))?);
        let assignment = assignments
            .get(&key)
            .await?
            .context("original assignment disappeared")?
            .to_vec();
        let commands = context.get_key_value("sandbox-e2b-command-custody").await?;
        let key = tenant_scoped_kv_key(
            &assigned.scope.tenant,
            &serde_json::to_string(&(assigned.lease_id.as_str(), OLD_KEY))?,
        );
        let command = serde_json::from_slice(
            &commands
                .get(&key)
                .await?
                .context("original command authority disappeared")?,
        )?;
        Ok((assignment, command))
    }

    pub async fn authority(&self, assigned: &Assigned) -> Result<OriginalAuthority, Boxed> {
        let (assignment, command) = self.authority_values(assigned).await?;
        assert!(
            command["phase"] == "running",
            "positive original custody must be Running"
        );
        Ok(OriginalAuthority {
            assignment,
            command,
            fence: std::fs::read(self.fence_path())?,
        })
    }

    pub async fn authority_preserved(&self, assigned: &Assigned, before: &OriginalAuthority) -> Result<bool, Boxed> {
        let (assignment, command) = self.authority_values(assigned).await?;
        Ok(assignment == before.assignment
            && command["phase"] == "canceled"
            && command["cancel_requested"] == true
            && ["owner", "execution_id", "fingerprint", "execution"]
                .into_iter()
                .all(|field| command[field] == before.command[field]))
    }

    pub async fn finish(mut self, assigned: Option<&Assigned>) -> Result<Cleanup, Boxed> {
        let native_created = self.api.control.creates.load(Ordering::SeqCst) != 0;
        let reply = if let Some(assigned) = assigned {
            let request = ProviderCleanupRequest {
                identity: LeaseAttemptIdentity {
                    lease_id: assigned.lease_id.clone(),
                    provider: ProviderKind::E2b,
                    target: assigned.target.clone(),
                },
                intent: CancelIntent::Release,
            };
            let response: Result<ProviderCleanupResponse, Boxed> = signed(
                &self.nats,
                &self.signer,
                subjects::worker_cleanup(ProviderKind::E2b, &assigned.target.worker_id),
                &request,
                &assigned.scope,
            )
            .await;
            matches!(response, Ok(ProviderCleanupResponse::Cleaned { .. }))
        } else {
            false
        };
        let removed = self.api.control.product_removed.load(Ordering::SeqCst);
        let product = reply && removed;
        self.worker.stop()?;
        self.native.shutdown().await;
        self.api.shutdown().await;
        self.envd.shutdown().await;
        if !removed {
            self.guest.cleanup()?;
        }
        Ok(Cleanup {
            product,
            native_created,
        })
    }
}

pub struct FenceSwap {
    canonical: PathBuf,
    retained: PathBuf,
    replacement: PathBuf,
    original: (u64, u64),
    active: bool,
}

impl FenceSwap {
    pub fn replace(path: &Path) -> Result<Self, Boxed> {
        let metadata = std::fs::metadata(path)?;
        let retained = path.with_extension("retained-fence");
        let replacement = path.with_extension("replacement-fence");
        std::fs::rename(path, &retained)?;
        let state = Self {
            canonical: path.to_owned(),
            retained,
            replacement,
            original: (metadata.dev(), metadata.ino()),
            active: true,
        };
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        let fresh = std::fs::metadata(path)?;
        assert!(
            (fresh.dev(), fresh.ino()) != state.original,
            "canonical fence replacement must be a distinct inode"
        );
        Ok(state)
    }

    pub fn original_bytes(&self) -> Result<Vec<u8>, Boxed> {
        Ok(std::fs::read(&self.retained)?)
    }

    pub fn replacement_empty(&self) -> Result<bool, Boxed> {
        Ok(std::fs::read(&self.canonical)?.is_empty())
    }

    pub fn restore(&mut self) -> Result<(), Boxed> {
        if !self.active {
            return Ok(());
        }
        std::fs::rename(&self.canonical, &self.replacement)?;
        std::fs::rename(&self.retained, &self.canonical)?;
        let restored = std::fs::metadata(&self.canonical)?;
        assert!(
            (restored.dev(), restored.ino()) == self.original,
            "restore must return the original journal inode"
        );
        self.active = false;
        std::fs::remove_file(&self.replacement)?;
        Ok(())
    }
}

impl Drop for FenceSwap {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
