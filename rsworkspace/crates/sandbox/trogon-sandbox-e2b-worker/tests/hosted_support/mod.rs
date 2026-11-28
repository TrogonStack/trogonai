//! Shared by the hosted lanes: a worker already running against a real E2B account, reached over NATS.
//!
//! A lane runs only when `SANDBOX_E2B_HOSTED_TEST=1`. It needs these variables, which are read by name and never
//! printed:
//!
//! - `SANDBOX_CONFORMANCE_NATS_URL`: the NATS server the worker is connected to.
//! - `SANDBOX_CONFORMANCE_WORKER_ID`: optional, pins the worker whose heartbeat is used.
//! - `SANDBOX_CONFORMANCE_IMAGE`: optional, the template the conformance leases request (default `base`).
//! - `E2B_API_KEY`, and optionally `E2B_API_URL` and `E2B_DOMAIN`.
//! - the dispatch signing key variables the worker verifies, when it requires signed requests.
// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::time::{Duration, SystemTime};

use async_nats::Client;
use futures::StreamExt;
use serde_json::json;
use trogon_sandbox_conformance::{GrantLedger, NativeCleanupAuthority, Target, run_suite, signer_from_env};
use trogon_sandbox_contract::{ProviderKind, WorkerHeartbeat};
use trogon_sandbox_e2b_worker::{ApiError, DEFAULT_API_URL, DEFAULT_DOMAIN, E2bApi, E2bConfig};
use trogon_sandbox_protocol::subjects;
use trogon_std::env::{ReadEnv, SystemEnv};
use url::Url;

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult = Result<(), Boxed>;

pub const GATE: &str = "SANDBOX_E2B_HOSTED_TEST";

/// How long one lane may run end to end.
pub const LANE_BOUND: Duration = Duration::from_secs(900);
const HEARTBEAT_WAIT: Duration = Duration::from_secs(30);
const MAX_HEARTBEAT_AGE: Duration = Duration::from_secs(30);
const ABSENCE_WAIT: Duration = Duration::from_secs(15);
const ABSENCE_POLL: Duration = Duration::from_millis(500);
const REQUEST_BOUND: Duration = Duration::from_secs(20);

pub fn var(name: &str) -> Result<String, Boxed> {
    SystemEnv
        .var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is not set").into())
}

pub fn optional(name: &str) -> Option<String> {
    SystemEnv.var(name).ok().filter(|value| !value.is_empty())
}

/// Fails unless the lane was authorized with `gate=1`.
pub fn require_gate(gate: &str) -> TestResult {
    if SystemEnv.var(gate).as_deref() == Ok("1") {
        Ok(())
    } else {
        Err(format!("{gate}=1 is required for this hosted lane").into())
    }
}

/// The E2B account the lane runs against, configured from the environment.
pub fn e2b_config() -> Result<E2bConfig, Boxed> {
    let url = optional("E2B_API_URL").unwrap_or_else(|| DEFAULT_API_URL.to_owned());
    let domain = optional("E2B_DOMAIN").unwrap_or_else(|| DEFAULT_DOMAIN.to_owned());
    Ok(E2bConfig::new(Url::parse(&url)?, domain, var("E2B_API_KEY")?)?)
}

/// A client for the account the lane runs against.
pub fn e2b_api() -> Result<E2bApi, Boxed> {
    Ok(E2bApi::new(e2b_config()?, REQUEST_BOUND)?)
}

/// Reads the sandbox records of the account with its own client, so a lane checks the provider rather than trusting
/// the worker.
pub struct Native {
    api: E2bApi,
}

impl Native {
    pub fn from_env() -> Result<Self, Boxed> {
        Ok(Self { api: e2b_api()? })
    }

    pub async fn absent(&self, id: &str) -> Result<bool, Boxed> {
        match self.api.get_sandbox(id).await {
            Err(ApiError::NotFound(_)) => Ok(true),
            Ok(_) => Ok(false),
            Err(error) => Err(format!("reading a sandbox record failed: {error}").into()),
        }
    }

    /// Waits for the sandbox to be gone, deleting it once if it is not.
    pub async fn settle_absence(&self, id: &str) -> Result<(), Boxed> {
        if self.wait_absent(id).await? {
            return Ok(());
        }
        self.api
            .delete_sandbox(id)
            .await
            .map_err(|error| format!("deleting a sandbox failed: {error}"))?;
        if self.wait_absent(id).await? {
            Ok(())
        } else {
            Err(format!("sandbox {id} is still present after deletion").into())
        }
    }

    async fn wait_absent(&self, id: &str) -> Result<bool, Boxed> {
        let deadline = tokio::time::Instant::now() + ABSENCE_WAIT;
        loop {
            if self.absent(id).await? {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(ABSENCE_POLL).await;
        }
    }
}

/// A worker found by its heartbeat, with what a lane needs to reach it.
pub struct Harness {
    pub client: Client,
    pub heartbeat: WorkerHeartbeat,
    pub native: Native,
    pub image: String,
}

impl Harness {
    /// Connects to the worker, or fails with the name of what is missing.
    pub async fn configured() -> Result<Self, Boxed> {
        require_gate(GATE)?;
        let native = Native::from_env()?;
        let client = async_nats::connect(var("SANDBOX_CONFORMANCE_NATS_URL")?).await?;
        let pinned = optional("SANDBOX_CONFORMANCE_WORKER_ID");
        let heartbeat = observe_worker(&client, pinned.as_deref()).await?;
        Ok(Self {
            client,
            heartbeat,
            native,
            image: optional("SANDBOX_CONFORMANCE_IMAGE").unwrap_or_else(|| "base".to_owned()),
        })
    }

    /// The conformance target for the observed worker, recording every grant in `ledger`. Which optional checks run
    /// follows what the worker's own profile advertises.
    pub fn target(&self, ledger: GrantLedger) -> Result<Target, Boxed> {
        let mut target = Target::new(self.client.clone(), ProviderKind::E2b)
            .for_worker(&self.heartbeat)?
            .with_grant_ledger(ledger);
        target.signer = signer_from_env(&SystemEnv)?;
        target.image = self.image.clone();
        target.timeout = Duration::from_secs(120);
        let profile = &self.heartbeat.profile;
        target.supports_pause_resume = profile.lifecycle.supports_pause_resume.is_supported();
        target.graceful_stop = profile.isolation.supports_graceful_stop.is_supported();
        target.command_restart = profile.isolation.supports_command_restart.is_supported();
        target.named_processes = profile.isolation.supports_named_processes.is_supported();
        target.process_readiness = profile.isolation.supports_process_readiness.is_supported();
        target.checkpoint_running_unsupported = !profile.snapshot.can_checkpoint_running.is_supported();
        target.native_passthrough = profile.placement.supports_native_passthrough.is_supported();
        target.redispatch_idempotent = true;
        target.file_ops = profile.isolation.supports_file_ops.is_supported();
        target.file_search = profile.isolation.supports_file_search.is_supported();
        target.file_archive = profile.isolation.supports_file_archive.is_supported();
        target.denied_connect_fails = profile.isolation.egress.deny_all.is_supported();
        Ok(target)
    }
}

/// The first fresh heartbeat of an E2B worker, optionally of the worker named `pinned`.
pub async fn observe_worker(client: &Client, pinned: Option<&str>) -> Result<WorkerHeartbeat, Boxed> {
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    client.flush().await?;
    let deadline = tokio::time::Instant::now() + HEARTBEAT_WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, beats.next())
            .await
            .map_err(|_| "no worker heartbeat arrived in time")?
            .ok_or("the heartbeat subscription closed")?;
        let Ok(heartbeat) = serde_json::from_slice::<WorkerHeartbeat>(&message.payload) else {
            continue;
        };
        let pinned_elsewhere = pinned.is_some_and(|id| heartbeat.worker_id.as_str() != id);
        let stale = SystemTime::now()
            .duration_since(heartbeat.emitted_at)
            .is_ok_and(|age| age > MAX_HEARTBEAT_AGE);
        if heartbeat.provider == ProviderKind::E2b && !pinned_elsewhere && !stale {
            println!(
                "{}",
                json!({"receipt": "worker", "worker_id": heartbeat.worker_id.as_str(), "protocol": format!("{:?}", heartbeat.dispatch_protocol)})
            );
            return Ok(heartbeat);
        }
    }
}

/// Confirms every sandbox the ledger names is gone, deleting what is left, and returns what could not be settled.
pub async fn settle_ledger(native: &Native, ledger: &GrantLedger) -> (usize, Vec<String>) {
    let mut errors = Vec::new();
    let mut count = 0;
    let mut reconciliation_required = 0;
    for grant in ledger.snapshot() {
        count += 1;
        match grant.native_cleanup_authority {
            NativeCleanupAuthority::RequestMatched => {
                if let Err(error) = native.settle_absence(grant.native.sandbox_id.as_str()).await {
                    errors.push(error.to_string());
                }
            }
            NativeCleanupAuthority::ReconciliationRequired => reconciliation_required += 1,
        }
    }
    if reconciliation_required > 0 {
        errors.push(format!(
            "{reconciliation_required} grants carry no cleanup authority and need reconciliation"
        ));
    }
    (count, errors)
}

/// Runs the whole suite against `target`, then proves every sandbox the run created is gone.
pub async fn run_with_exact_cleanup(
    harness: &Harness,
    target: &Target,
    ledger: &GrantLedger,
    lane: &str,
) -> TestResult {
    let outcome = tokio::time::timeout(LANE_BOUND, run_suite(target)).await;
    let (grants, mut errors) = settle_ledger(&harness.native, ledger).await;
    if grants == 0 {
        errors.push("the run created no sandbox".to_owned());
    }
    let failures = outcome.as_ref().ok();
    let names: Vec<&str> = failures
        .map(|failures| failures.iter().map(|failure| failure.name).collect())
        .unwrap_or_default();
    println!(
        "{}",
        json!({
            "receipt": lane,
            "grants": grants,
            "timed_out": failures.is_none(),
            "failed_checks": names,
            "cleanup_errors": errors.len(),
        })
    );
    if failures.is_none() {
        return Err(format!("{lane} did not finish within its bound").into());
    }
    if !names.is_empty() {
        return Err(format!("hosted worker is not conformant: {}", names.join(", ")).into());
    }
    if let Some(error) = errors.first() {
        return Err(format!("cleanup was not exact: {error}").into());
    }
    Ok(())
}

pub mod owned_lease;
