//! Shared by the hosted lanes: a worker already running against a real Daytona account, reached over NATS.
//!
//! A lane runs only when `SANDBOX_DAYTONA_HOSTED_TEST=1`. It needs these variables, which are read by name and never
//! printed:
//!
//! - `SANDBOX_CONFORMANCE_NATS_URL`: the NATS server the worker is connected to.
//! - `SANDBOX_CONFORMANCE_WORKER_ID`: optional, pins the worker whose heartbeat is used.
//! - `SANDBOX_DAYTONA_SNAPSHOT`: the active snapshot the worker leases from.
//! - `DAYTONA_API_KEY`, `DAYTONA_ORGANIZATION_ID`, and optionally `DAYTONA_BASE_URL`.
//! - the dispatch signing key variables the worker verifies, when it requires signed requests.
// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::time::{Duration, SystemTime};

use async_nats::Client;
use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde_json::json;
use trogon_sandbox_conformance::{
    CheckFn, GrantLedger, NativeCleanupAuthority, SuiteFailureError, Target, run_suite, signer_from_env,
};
use trogon_sandbox_contract::{ProviderKind, WorkerHeartbeat};
use trogon_sandbox_daytona_worker::{
    DEFAULT_API_URL, DaytonaApi, DaytonaApiKey, DaytonaConfig, DaytonaEndpoint, DaytonaOrganizationId,
};
use trogon_sandbox_protocol::subjects;
use trogon_std::env::{ReadEnv, SystemEnv};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult = Result<(), Boxed>;

pub const GATE: &str = "SANDBOX_DAYTONA_HOSTED_TEST";
pub const IGNORE_REASON: &str =
    "runs against a real Daytona account; set SANDBOX_DAYTONA_HOSTED_TEST=1 and use --ignored";

/// How long one lane may run end to end.
pub const LANE_BOUND: Duration = Duration::from_secs(900);
const HEARTBEAT_WAIT: Duration = Duration::from_secs(30);
const MAX_HEARTBEAT_AGE: Duration = Duration::from_secs(30);
const ABSENCE_WAIT: Duration = Duration::from_secs(15);
const ABSENCE_POLL: Duration = Duration::from_millis(500);
const REQUEST_BOUND: Duration = Duration::from_secs(20);

pub fn var(name: &str) -> Result<String, Boxed> {
    SystemEnv.var(name).map_err(|_| format!("{name} is not set").into())
}

fn optional(name: &str) -> Option<String> {
    SystemEnv.var(name).ok().filter(|value| !value.is_empty())
}

/// Reads the sandbox records of the account the lane runs against, with its own client, so a lane checks the
/// provider rather than trusting the worker.
pub struct Native {
    http: reqwest::Client,
    base: String,
    auth: HeaderValue,
    organization: HeaderValue,
    pub organization_id: String,
}

impl Native {
    pub fn from_env() -> Result<Self, Boxed> {
        let key = var("DAYTONA_API_KEY")?;
        let organization_id = var("DAYTONA_ORGANIZATION_ID")?;
        let endpoint =
            DaytonaEndpoint::parse(&optional("DAYTONA_BASE_URL").unwrap_or_else(|| DEFAULT_API_URL.to_owned()))?;
        let mut auth = HeaderValue::from_str(&format!("Bearer {key}"))?;
        auth.set_sensitive(true);
        let mut organization = HeaderValue::from_str(&organization_id)?;
        organization.set_sensitive(true);
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(REQUEST_BOUND)
                .build()?,
            base: endpoint.as_str().trim_end_matches('/').to_owned(),
            auth,
            organization,
            organization_id,
        })
    }

    fn request(&self, method: reqwest::Method, id: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/sandbox/{id}", self.base))
            .header(AUTHORIZATION, self.auth.clone())
            .header("X-Daytona-Organization-ID", self.organization.clone())
    }

    /// The status and body of the sandbox record.
    pub async fn record(&self, id: &str) -> Result<(u16, Vec<u8>), Boxed> {
        let response = self.request(reqwest::Method::GET, id).send().await?;
        let status = response.status().as_u16();
        Ok((status, response.bytes().await?.to_vec()))
    }

    pub async fn absent(&self, id: &str) -> Result<bool, Boxed> {
        Ok(self.record(id).await?.0 == 404)
    }

    pub async fn delete(&self, id: &str) -> Result<(), Boxed> {
        let status = self.request(reqwest::Method::DELETE, id).send().await?.status();
        if status.is_success() || status.as_u16() == 404 {
            Ok(())
        } else {
            Err(format!("deleting a sandbox answered {status}").into())
        }
    }

    /// Waits for the sandbox to be gone, deleting it once if it is not.
    pub async fn settle_absence(&self, id: &str) -> Result<(), Boxed> {
        if self.wait_absent(id).await? {
            return Ok(());
        }
        self.delete(id).await?;
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

/// A client for the account `native` was configured with.
pub fn daytona_api(native: &Native) -> Result<DaytonaApi, Boxed> {
    let endpoint = DaytonaEndpoint::parse(&optional("DAYTONA_BASE_URL").unwrap_or_else(|| DEFAULT_API_URL.to_owned()))?;
    let config = DaytonaConfig::new(
        endpoint,
        DaytonaApiKey::new(&var("DAYTONA_API_KEY")?)?,
        Some(DaytonaOrganizationId::new(&native.organization_id)?),
    );
    Ok(DaytonaApi::new(config, REQUEST_BOUND)?)
}

/// A worker found by its heartbeat, with what a lane needs to reach it.
pub struct Harness {
    pub client: Client,
    pub heartbeat: WorkerHeartbeat,
    pub native: Native,
    pub api: DaytonaApi,
    pub snapshot: String,
}

impl Harness {
    /// Connects to the worker, or fails with the name of what is missing.
    pub async fn configured() -> Result<Self, Boxed> {
        if SystemEnv.var(GATE).as_deref() != Ok("1") {
            return Err(format!("{GATE}=1 is required for a hosted lane").into());
        }
        let native = Native::from_env()?;
        let snapshot = var("SANDBOX_DAYTONA_SNAPSHOT")?;
        let api = daytona_api(&native)?;
        let found = api.find_snapshot(&snapshot).await?;
        if !found.is_some_and(|record| matches!(record.state, trogon_sandbox_daytona_worker::SnapshotState::Active)) {
            return Err("the configured snapshot is not active".into());
        }
        let client = async_nats::connect(var("SANDBOX_CONFORMANCE_NATS_URL")?).await?;
        let pinned = optional("SANDBOX_CONFORMANCE_WORKER_ID");
        let heartbeat = observe_worker(&client, pinned.as_deref()).await?;
        Ok(Self {
            client,
            heartbeat,
            native,
            api,
            snapshot,
        })
    }

    /// The conformance target for the observed worker, recording every grant in `ledger`.
    pub fn target(&self, ledger: GrantLedger) -> Result<Target, Boxed> {
        let mut target = Target::new(self.client.clone(), ProviderKind::Daytona)
            .for_worker(&self.heartbeat)?
            .with_grant_ledger(ledger);
        target.signer = signer_from_env(&SystemEnv)?;
        target.image = self.snapshot.clone();
        target.timeout = Duration::from_secs(120);
        Ok(target)
    }
}

/// The first fresh heartbeat of a Daytona worker, optionally of the worker named `pinned`.
pub async fn observe_worker(client: &Client, pinned: Option<&str>) -> Result<WorkerHeartbeat, Boxed> {
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Daytona))
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
        if heartbeat.provider == ProviderKind::Daytona && !pinned_elsewhere && !stale {
            println!(
                "{}",
                json!({"receipt": "worker", "worker_id": heartbeat.worker_id.as_str(), "protocol": format!("{:?}", heartbeat.dispatch_protocol)})
            );
            return Ok(heartbeat);
        }
    }
}

/// Every grant the ledger holds, split by whether the lane may delete the sandbox.
pub struct Grants {
    pub owned: Vec<String>,
    pub reconciliation_required: usize,
}

pub fn grants(ledger: &GrantLedger) -> Grants {
    let mut owned = Vec::new();
    let mut reconciliation_required = 0;
    for grant in ledger.snapshot() {
        match grant.native_cleanup_authority {
            NativeCleanupAuthority::RequestMatched => owned.push(grant.native.sandbox_id.as_str().to_owned()),
            NativeCleanupAuthority::ReconciliationRequired => reconciliation_required += 1,
        }
    }
    Grants {
        owned,
        reconciliation_required,
    }
}

/// Confirms every sandbox the lane owns is gone, deleting what is left, and returns what could not be settled.
pub async fn settle_ledger(native: &Native, ledger: &GrantLedger) -> (usize, Vec<String>) {
    let held = grants(ledger);
    let mut errors = Vec::new();
    if held.reconciliation_required > 0 {
        errors.push(format!(
            "{} grants carry no cleanup authority and need reconciliation",
            held.reconciliation_required
        ));
    }
    for id in &held.owned {
        if let Err(error) = native.settle_absence(id).await {
            errors.push(error.to_string());
        }
    }
    (held.owned.len() + held.reconciliation_required, errors)
}

/// Runs `checks` against `target`, then proves every sandbox the run created is gone.
pub async fn run_checks_with_exact_cleanup(
    harness: &Harness,
    target: &Target,
    ledger: &GrantLedger,
    checks: Vec<(&'static str, CheckFn)>,
    lane: &str,
) -> TestResult {
    let outcome = tokio::time::timeout(LANE_BOUND, async {
        let mut failures: Vec<SuiteFailureError> = Vec::new();
        for (name, check) in checks {
            if let Err(failure) = check(target).await {
                failures.push(SuiteFailureError { name, failure });
            }
        }
        failures
    })
    .await;
    finish(harness, ledger, lane, outcome.ok()).await
}

/// Runs the whole suite against `target`, then proves every sandbox the run created is gone.
pub async fn run_with_exact_cleanup(
    harness: &Harness,
    target: &Target,
    ledger: &GrantLedger,
    lane: &str,
) -> TestResult {
    let outcome = tokio::time::timeout(LANE_BOUND, run_suite(target)).await;
    finish(harness, ledger, lane, outcome.ok()).await
}

pub async fn finish(
    harness: &Harness,
    ledger: &GrantLedger,
    lane: &str,
    failures: Option<Vec<SuiteFailureError>>,
) -> TestResult {
    let (grant_count, mut errors) = settle_ledger(&harness.native, ledger).await;
    if grant_count == 0 {
        errors.push("the run created no sandbox".to_owned());
    }
    let names: Vec<&str> = failures
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|failure| failure.name)
        .collect();
    let markers = failures
        .as_deref()
        .map(crate::suite_failure_diagnosis::diagnose)
        .unwrap_or_default();
    println!(
        "{}",
        json!({
            "receipt": lane,
            "grants": grant_count,
            "timed_out": failures.is_none(),
            "failed_checks": names,
            "markers": markers,
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
