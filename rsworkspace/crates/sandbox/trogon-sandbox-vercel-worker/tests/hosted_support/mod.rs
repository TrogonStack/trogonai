//! Shared by the hosted lanes, which run against a real Vercel team.
//!
//! A lane runs only when `SANDBOX_VERCEL_HOSTED_TEST=1`, and a lane that stores snapshots also needs
//! `SANDBOX_VERCEL_PAID_TEST=1`. The credentials are read by name and never printed:
//!
//! - `VERCEL_TOKEN`, `VERCEL_TEAM_ID` and `VERCEL_PROJECT_ID`, and optionally `SANDBOX_VERCEL_API_URL`.
//! - `SANDBOX_CONFORMANCE_NATS_URL`: the NATS server a running worker of the same team is connected to. Only the
//!   conformance lane needs it, and `SANDBOX_CONFORMANCE_WORKER_ID` optionally pins the worker.
//! - the dispatch signing key variables the worker verifies, when it requires signed requests.
// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

pub mod snapshot_retirement;

use std::time::Duration;

use trogon_sandbox_vercel_worker::{
    DEFAULT_API_URL, VercelApi, VercelApiError, VercelConfig, VercelEndpoint, VercelProjectId, VercelTeamId,
    VercelToken,
};
use trogon_std::env::{ReadEnv, SystemEnv};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult = Result<(), Boxed>;

pub const GATE: &str = "SANDBOX_VERCEL_HOSTED_TEST";
pub const PAID_GATE: &str = "SANDBOX_VERCEL_PAID_TEST";

/// How long a native check waits for the account to show what a lane just did.
const SETTLE_WAIT: Duration = Duration::from_secs(20);
const SETTLE_POLL: Duration = Duration::from_millis(250);

pub fn var(name: &str) -> Result<String, Boxed> {
    SystemEnv
        .var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} must be set").into())
}

pub fn optional(name: &str) -> Option<String> {
    SystemEnv.var(name).ok().filter(|value| !value.is_empty())
}

/// Fails with the name of the gate unless it is `1`.
pub fn require(gate: &str) -> TestResult {
    if SystemEnv.var(gate).as_deref() == Ok("1") {
        Ok(())
    } else {
        Err(format!("set {gate}=1 to authorize this hosted lane").into())
    }
}

pub fn endpoint() -> Result<VercelEndpoint, Boxed> {
    let url = optional("SANDBOX_VERCEL_API_URL").unwrap_or_else(|| DEFAULT_API_URL.to_owned());
    Ok(VercelEndpoint::parse(&url)?)
}

pub fn config() -> Result<VercelConfig, Boxed> {
    Ok(VercelConfig::new(
        endpoint()?,
        VercelTeamId::new(var("VERCEL_TEAM_ID")?)?,
        VercelProjectId::new(var("VERCEL_PROJECT_ID")?)?,
        VercelToken::new(&var("VERCEL_TOKEN")?)?,
    ))
}

/// A client of the team the lane runs against, with its own request bound.
pub fn api(timeout: Duration) -> Result<VercelApi, Boxed> {
    Ok(VercelApi::new(config()?, timeout)?)
}

/// Whether `value` is a complete identity the lane may act on: non-empty, bounded and made of safe characters only.
pub fn exact_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Waits until the native record of `name` shows exactly `session` running, so an absence check made later has a
/// positive control from the same account.
pub async fn verify_running(api: &VercelApi, name: &str, session: &str) -> TestResult {
    if !exact_id(name) || !exact_id(session) {
        return Err("the native allocation carries no complete exact identity".into());
    }
    tokio::time::timeout(SETTLE_WAIT, async {
        loop {
            let found = api
                .get_sandbox(name, false)
                .await
                .map_err(|_| "the native GET failed, so absence checks have no positive account control")?;
            if found.sandbox.name != name || found.session.id != session {
                return Err("the native GET answered for another sandbox or session".into());
            }
            if found.session.status == "running" {
                return Ok(());
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
    })
    .await
    .map_err(|_| -> Boxed { "the native record did not show a running session in time".into() })?
}

/// Waits until the native GET of `name` is a not-found. Any other failure leaves absence unknown.
pub async fn confirm_absent(api: &VercelApi, name: &str) -> TestResult {
    tokio::time::timeout(SETTLE_WAIT, async {
        loop {
            match api.get_sandbox(name, false).await {
                Err(VercelApiError::NotFound(_)) => return Ok(()),
                Ok(_) => tokio::time::sleep(SETTLE_POLL).await,
                Err(_) => return Err::<(), Boxed>("native absence stays unknown after a failed GET".into()),
            }
        }
    })
    .await
    .map_err(|_| -> Boxed { "the sandbox is still present after cleanup".into() })?
}

/// Confirms `name` is gone, deleting it once when it is not.
pub async fn settle_absence(api: &VercelApi, name: &str) -> TestResult {
    if confirm_absent(api, name).await.is_ok() {
        return Ok(());
    }
    api.delete_sandbox(name).await?;
    confirm_absent(api, name).await
}
