//! A forced snapshot polling timeout against the real account and a real JetStream, and the exact cleanup after it.
//!
//! The lane serves its own worker, so it needs a dedicated NATS server: a second worker on a shared one would compete
//! for the hosted worker's requests. It proves that a capture which timed out stays owned by durable custody, that
//! the source is still usable, and that a public cancel retires the source and the unpublished snapshot while a
//! published snapshot and an unrelated sandbox survive.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, LeaseId, ProviderKind, SandboxImage, SnapshotExportError,
    SnapshotExportOutcome, SnapshotPushTarget, WorkerId,
};
use trogon_sandbox_daytona_worker::{
    DaytonaExec, DaytonaWorkerProvider, ExecuteRequest, SnapshotCustody, SnapshotState, SnapshotWait,
};
use trogon_sandbox_protocol::subjects::provider_checkpoint_running;
use trogon_sandbox_protocol::{Provider, nats_transfer};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::hosted_support::{Boxed, GATE, LANE_BOUND, Native, TestResult, daytona_api, var};
use crate::session::lease_request;

const CALL: Duration = Duration::from_secs(120);
const CUSTODY_BUCKET: &str = "sandbox-daytona-snapshot-custody";
const POLL: Duration = Duration::from_millis(250);

#[derive(Deserialize)]
struct Capture {
    id: Option<String>,
    kept: bool,
    deleted: bool,
    #[serde(default)]
    pre_create_absent: bool,
    #[serde(default)]
    create_acknowledged: bool,
}

#[derive(Deserialize)]
struct Custody {
    source: String,
    organization: Option<String>,
    closed: bool,
    captures: BTreeMap<String, Capture>,
}

fn fail<T>(reason: &str) -> Result<T, Boxed> {
    Err(reason.into())
}

struct Focus {
    native: Native,
    api: trogon_sandbox_daytona_worker::DaytonaApi,
    js: jetstream::Context,
    client: async_nats::Client,
    provider: Arc<DaytonaWorkerProvider>,
    image: String,
    origin: String,
    leases: [LeaseId; 2],
    sandboxes: [Option<String>; 2],
    published: Option<String>,
    failed_name: String,
    checks: Vec<&'static str>,
}

impl Focus {
    async fn acquire(&mut self, slot: usize) -> Result<(), Boxed> {
        let image = SandboxImage::Registry {
            reference: trogon_sandbox_contract::OciImageRef::parse_unqualified(&self.image),
            pull_secret: None,
        };
        let grant = self.provider.lease(&lease_request(image)?, &self.leases[slot]).await?;
        let id = grant.sandbox_id.as_str().to_owned();
        let record = self
            .api
            .find_sandbox(&id)
            .await?
            .ok_or("the granted sandbox is not visible to the account")?;
        if record.id != id || record.organization_id.as_deref() != Some(self.native.organization_id.as_str()) {
            return fail("the granted sandbox is not in the lane's account");
        }
        self.sandboxes[slot] = Some(id);
        Ok(())
    }

    fn source(&self) -> Result<&str, Boxed> {
        self.sandboxes[0]
            .as_deref()
            .ok_or_else(|| "the source sandbox was never granted".into())
    }

    async fn custody(&self) -> Result<Custody, Boxed> {
        let source = self.source()?;
        let store = self.js.get_key_value(CUSTODY_BUCKET).await?;
        let value = store
            .get(format!("{}.{source}", self.origin))
            .await?
            .ok_or("the durable custody record is missing")?;
        let record: Custody = serde_json::from_slice(&value)?;
        if record.source != source || record.organization.as_deref() != Some(self.native.organization_id.as_str()) {
            return fail("the custody record names a different source or account");
        }
        Ok(record)
    }

    async fn capture(&self, name: &str) -> Result<SnapshotExportOutcome, Boxed> {
        let request = CheckpointRunningRequest {
            lease_id: self.leases[0].clone(),
            target: SnapshotPushTarget {
                registry: "unused.invalid".into(),
                repository: "unused".into(),
                artifact_type: ArtifactType::SandboxRootfsV1,
            },
            native_name: Some(name.to_owned()),
        };
        let reply = nats_transfer::request(
            &self.client,
            &provider_checkpoint_running(ProviderKind::Daytona, &self.leases[0]),
            serde_json::to_vec(&request)?.into(),
            None,
            CALL,
        )
        .await?;
        Ok(serde_json::from_slice(&reply)?)
    }

    /// The snapshot called `name`, once it is owned by the source in the lane's account.
    async fn owned_snapshot(&self, name: &str) -> Result<trogon_sandbox_daytona_worker::SnapshotRecord, Boxed> {
        let record = self
            .api
            .find_snapshot(name)
            .await?
            .ok_or("the snapshot is not visible")?;
        let owned = record.name == name
            && !record.general
            && record.source_sandbox_id.as_deref() == Some(self.source()?)
            && record.organization_id.as_deref() == Some(self.native.organization_id.as_str());
        if owned {
            Ok(record)
        } else {
            fail("the snapshot's name, source or account differs")
        }
    }

    async fn unpublished_visible(&self, until: Instant) -> Result<String, Boxed> {
        loop {
            if let Some(record) = self.api.find_snapshot(&self.failed_name).await? {
                let owned = self.owned_snapshot(&self.failed_name).await?;
                let custody = self.custody().await?;
                let capture = custody
                    .captures
                    .get(&self.failed_name)
                    .ok_or("the unpublished intent is missing")?;
                let held = !custody.closed
                    && !capture.kept
                    && !capture.deleted
                    && capture.pre_create_absent
                    && capture.create_acknowledged
                    && capture.id.as_deref().is_none_or(|id| id == owned.id);
                if !held {
                    return fail("custody does not hold the unpublished capture");
                }
                return Ok(record.id);
            }
            if Instant::now() >= until {
                return fail("the unpublished capture never became visible");
            }
            tokio::time::sleep(POLL).await;
        }
    }

    async fn work(&mut self) -> Result<(), Boxed> {
        self.acquire(0).await?;
        self.acquire(1).await?;
        self.checks.push("source_and_control_in_the_lane_account");
        let source = self.source()?.to_owned();

        let published = format!("sdk-published-{}", uuid::Uuid::now_v7().simple());
        self.published = Some(published.clone());
        let SnapshotExportOutcome::Native(receipt) = self.capture(&published).await? else {
            return fail("the published capture did not return a native receipt");
        };
        if receipt.reference.id.as_str() != published {
            return fail("the receipt names a different snapshot");
        }
        self.owned_snapshot(&published).await?;
        let custody = self.custody().await?;
        if !custody
            .captures
            .get(&published)
            .is_some_and(|capture| capture.kept && !capture.deleted && capture.id.is_some())
        {
            return fail("the published capture is not kept in custody");
        }
        self.checks.push("published_receipt_and_custody_binding");

        self.provider.set_snapshot_wait(SnapshotWait {
            poll_interval: Duration::from_millis(1),
            timeout: Duration::from_millis(1),
        });
        let prefix = format!("Daytona snapshot {} not active in time:", self.failed_name);
        let outcome = self.capture(&self.failed_name.clone()).await?;
        let SnapshotExportOutcome::Err(SnapshotExportError::Transient { message }) = outcome else {
            return fail("the forced timeout did not end as a transient failure");
        };
        if !message.starts_with(&prefix) {
            return fail("the transient failure is not the polling timeout");
        }
        let until = Instant::now() + CALL;
        let unpublished = self.unpublished_visible(until).await?;
        self.checks.push("poll_timeout_with_unpublished_custody");

        let usable = self
            .api
            .execute(
                &source,
                &ExecuteRequest {
                    command: "printf source-survived-timeout".into(),
                    cwd: None,
                    timeout: Some(10),
                },
            )
            .await?;
        if usable.exit_code != 0 || !usable.result.contains("source-survived-timeout") {
            return fail("the source stopped working after the timeout");
        }
        self.checks.push("source_usable_after_timeout");

        loop {
            if self.provider.cancel(&self.leases[0]).await.is_ok() {
                break;
            }
            if Instant::now() >= until {
                return fail("the public cancel did not succeed within its bound");
            }
            tokio::time::sleep(POLL).await;
        }
        loop {
            let source_gone = self.api.find_sandbox(&source).await?.is_none();
            let capture_gone = self.api.find_snapshot(&unpublished).await?.is_none();
            if source_gone && capture_gone {
                break;
            }
            if Instant::now() >= until {
                return fail("the cancel did not retire the source and the unpublished snapshot");
            }
            tokio::time::sleep(POLL).await;
        }
        self.checks.push("cancel_retired_source_and_unpublished_snapshot");

        self.owned_snapshot(&published).await?;
        let control = self.sandboxes[1]
            .as_deref()
            .ok_or("the control sandbox was never granted")?;
        if self.api.find_sandbox(control).await?.is_none() {
            return fail("the cancel retired the unrelated sandbox");
        }
        let custody = self.custody().await?;
        let failed = custody.captures.get(&self.failed_name);
        let kept = custody.captures.get(&published);
        let settled = custody.closed
            && failed.is_some_and(|capture| capture.deleted && !capture.kept)
            && kept.is_some_and(|capture| capture.kept && !capture.deleted);
        if !settled {
            return fail("custody did not settle the deletion and the publication");
        }
        self.checks.push("published_snapshot_and_control_preserved");
        self.checks.push("custody_closed_with_exact_settlement");
        Ok(())
    }

    /// Retires what the lane still owns and returns the failures.
    async fn cleanup(&mut self) -> Vec<&'static str> {
        let mut errors = Vec::new();
        for slot in [0, 1] {
            let Some(id) = self.sandboxes[slot].clone() else {
                continue;
            };
            let _ = self.provider.cancel(&self.leases[slot]).await;
            if self.native.settle_absence(&id).await.is_err() {
                errors.push("sandbox_cleanup_unresolved");
            }
        }
        if let Some(name) = self.published.clone() {
            let retired = match self.owned_snapshot(&name).await {
                Ok(record) => self.api.delete_snapshot(&record.id).await.is_ok(),
                Err(_) => self.api.find_snapshot(&name).await.is_ok_and(|found| found.is_none()),
            };
            if !retired {
                errors.push("published_snapshot_retirement_unresolved");
            }
        }
        match self.api.find_snapshot(&self.image).await {
            Ok(Some(record)) if record.state == SnapshotState::Active => self.checks.push("fixture_snapshot_preserved"),
            _ => errors.push("fixture_snapshot_unresolved"),
        }
        errors
    }
}

pub async fn run() -> TestResult {
    if SystemEnv.var(GATE).as_deref() != Ok("1") {
        return Err(format!("{GATE}=1 is required for a hosted lane").into());
    }
    let native = Native::from_env()?;
    let image = var("SANDBOX_DAYTONA_SNAPSHOT")?;
    let api = daytona_api(&native)?;
    if !api
        .find_snapshot(&image)
        .await?
        .is_some_and(|record| record.state == SnapshotState::Active)
    {
        return fail("the configured snapshot is not active");
    }
    let client = async_nats::connect(var("SANDBOX_CONFORMANCE_NATS_URL")?).await?;
    let js = jetstream::new(client.clone());
    let origin = hex::encode(Sha256::digest(api.config().endpoint().as_str().as_bytes()));
    let provider = Arc::new(
        DaytonaWorkerProvider::new(
            daytona_api(&native)?,
            image.clone(),
            Arc::new(DaytonaExec::new(daytona_api(&native)?)),
        )
        .with_snapshot_custody(SnapshotCustody::new(js.clone(), &api)),
    );
    let cancel = CancellationToken::new();
    let worker_id = WorkerId::new(format!("daytona-timeout-{}", uuid::Uuid::now_v7().simple()))?;
    let served = tokio::spawn({
        let (client, js, provider, cancel) = (client.clone(), js.clone(), provider.clone(), cancel.clone());
        async move { trogon_sandbox_daytona_worker::run(client, js, provider, &worker_id, cancel).await }
    });
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut focus = Focus {
        native,
        api,
        js,
        client,
        provider,
        image,
        origin,
        leases: [
            LeaseId::new(uuid::Uuid::now_v7().to_string()),
            LeaseId::new(uuid::Uuid::now_v7().to_string()),
        ],
        sandboxes: [None, None],
        published: None,
        failed_name: format!("sdk-timeout-{}", uuid::Uuid::now_v7().simple()),
        checks: Vec::new(),
    };
    let outcome = tokio::time::timeout(LANE_BOUND, focus.work()).await;
    let errors = focus.cleanup().await;
    cancel.cancel();
    let _ = served.await;
    println!(
        "{}",
        json!({
            "receipt": "snapshot_timeout_cleanup",
            "checks": focus.checks,
            "sandboxes": focus.sandboxes,
            "cleanup_errors": errors,
        })
    );
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("the timeout lane failed: {error}").into()),
        Err(_) => return Err("the timeout lane did not finish within its bound".into()),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        fail("cleanup was not exact")
    }
}
