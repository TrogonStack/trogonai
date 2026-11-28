//! A lease the lane holds directly through the provider, against a real E2B account, with exact cleanup.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use trogon_sandbox_contract::{CommandOutput, LeaseId, LeaseRequest, RunCommandRequest, RunCommandResponse};
use trogon_sandbox_e2b_worker::{ApiError, E2bApi, E2bExec, E2bWorkerProvider, Envd, LeaseSandbox};
use trogon_sandbox_protocol::Provider;

use super::{Boxed, TestResult, e2b_api, optional, require_gate};

const COMMAND_BOUND: Duration = Duration::from_secs(35);
const ABSENCE_WAIT: Duration = Duration::from_secs(15);

/// The tenant every lease of this lane is requested under.
pub const TENANT: &str = "hosted-probe";

pub struct OwnedLease {
    pub api: E2bApi,
    pub provider: Arc<E2bWorkerProvider>,
    pub lease_id: LeaseId,
    pub sandbox_id: Option<String>,
}

impl OwnedLease {
    /// A provider on the account named by the environment, for a lane authorized by `gate`.
    pub fn new(gate: &str, prefix: &str) -> Result<Self, Boxed> {
        require_gate(gate)?;
        let api = e2b_api()?;
        let template = optional("SANDBOX_E2B_TEMPLATE").unwrap_or_else(|| "base".to_owned());
        let exec = Arc::new(E2bExec::new(Envd::new(Duration::from_secs(10))?));
        Ok(Self {
            provider: Arc::new(E2bWorkerProvider::new(api.clone(), template, exec)),
            api,
            lease_id: LeaseId::new(format!("{prefix}-{}", uuid::Uuid::new_v4())),
            sandbox_id: None,
        })
    }

    /// Another lease on the same account and provider, so a restore reaches the provider that holds the custody.
    pub fn sharing(&self, prefix: &str) -> Self {
        Self {
            api: self.api.clone(),
            provider: Arc::clone(&self.provider),
            lease_id: LeaseId::new(format!("{prefix}-{}", uuid::Uuid::new_v4())),
            sandbox_id: None,
        }
    }

    /// Leases one sandbox of the configured template for `ttl_seconds` and returns the handle every per-lease verb needs.
    pub async fn lease(&mut self, ttl_seconds: u32) -> Result<LeaseSandbox, Boxed> {
        let template = optional("SANDBOX_E2B_TEMPLATE").unwrap_or_else(|| "base".to_owned());
        let image = json!({"kind": "registry", "reference": {"registry": "", "repository": template}});
        self.lease_from(ttl_seconds, image).await
    }

    /// Like [`Self::lease`], booting from `image`, the wire form of a sandbox image.
    pub async fn lease_from(&mut self, ttl_seconds: u32, image: serde_json::Value) -> Result<LeaseSandbox, Boxed> {
        let request: LeaseRequest = serde_json::from_value(json!({
            "tenant": TENANT,
            "provider_pref": ["e2b"],
            "spec": {
                "image": image,
                "workspace": {"empty": {}},
                "cpu_seconds": 10,
                "memory_mib": 512,
                "env": {},
            },
            "ttl_seconds": ttl_seconds,
            "idempotency_key": self.lease_id.as_str(),
        }))?;
        let granted = self.provider.lease(&request, &self.lease_id).await?;
        let id = granted.sandbox_id.as_str().to_owned();
        self.sandbox_id = Some(id.clone());
        eprintln!("owned hosted E2B sandbox: {id}");
        if self.api.get_sandbox(&id).await?.state != "running" {
            return Err("the created hosted sandbox must be running".into());
        }
        self.sandbox().await
    }

    /// The handle for the held lease, which must address the exact sandbox this lane created.
    pub async fn sandbox(&self) -> Result<LeaseSandbox, Boxed> {
        let sandbox = self
            .provider
            .sandbox_for_lease(&self.lease_id)
            .await
            .ok_or("the provider must retain the created lease")?;
        if Some(&sandbox.sandbox_id) != self.sandbox_id.as_ref() {
            return Err("per-lease verbs must address the exact created sandbox".into());
        }
        Ok(sandbox)
    }

    /// Runs `script` under bash and returns its settled output.
    pub async fn command(
        &self,
        sandbox: &LeaseSandbox,
        script: &str,
        timeout_seconds: u32,
    ) -> Result<CommandOutput, Boxed> {
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id": self.lease_id,
            "command": ["bash", "-c", script],
            "timeout_seconds": timeout_seconds,
            "deadline": "timeout",
            "max_output_bytes": 4096,
        }))?;
        let response = tokio::time::timeout(
            COMMAND_BOUND,
            self.provider.exec().handle_run_command(sandbox, &request),
        )
        .await
        .map_err(|_| "a foreground command did not settle within the hosted probe bound")?;
        match response {
            RunCommandResponse::Completed(output) => Ok(output),
            other => Err(format!("a foreground command did not complete: {other:?}").into()),
        }
    }

    /// Like [`Self::command`], and requires a zero exit without timeout or truncation.
    pub async fn succeeding(&self, sandbox: &LeaseSandbox, script: &str) -> Result<CommandOutput, Boxed> {
        let output = self.command(sandbox, script, 10).await?;
        if output.exit_code != 0 || !output.timed_out.is_within_deadline() || !output.truncated.is_complete() {
            return Err(format!("a hosted command failed: {output:?}").into());
        }
        Ok(output)
    }

    /// Releases the lease, deleting the sandbox directly if the release failed, and proves the exact sandbox is gone.
    pub async fn cleanup(&self) -> TestResult {
        let released = self.provider.cancel(&self.lease_id).await;
        let Some(id) = &self.sandbox_id else {
            return released.map_err(|error| format!("cleanup after a failed lease: {error}").into());
        };
        if released.is_err() {
            self.api.delete_sandbox(id).await?;
        }
        tokio::time::timeout(ABSENCE_WAIT, async {
            loop {
                match self.api.get_sandbox(id).await {
                    Err(ApiError::NotFound(_)) => return Ok::<(), Boxed>(()),
                    Ok(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                    Err(error) => return Err(error.into()),
                }
            }
        })
        .await
        .map_err(|_| "the exact native read did not confirm the owned sandbox absent")??;
        released.map_err(|error| format!("the provider release failed despite exact native cleanup: {error}"))?;
        if self.provider.sandbox_for_lease(&self.lease_id).await.is_some() {
            return Err("the provider must forget the deleted lease".into());
        }
        eprintln!("owned hosted E2B sandbox cleanup confirmed: {id}");
        Ok(())
    }
}

/// Cleans up after `outcome`, reporting both failures when both happen.
pub async fn finish(lease: &OwnedLease, outcome: std::thread::Result<TestResult>) -> TestResult {
    let cleanup = lease.cleanup().await;
    match outcome {
        Ok(Ok(())) => cleanup,
        Ok(Err(error)) => match cleanup {
            Ok(()) => Err(format!("the hosted probe failed; exact native cleanup confirmed: {error}").into()),
            Err(cleanup) => Err(format!("the hosted probe failed: {error}; cleanup also failed: {cleanup}").into()),
        },
        Err(panic) => {
            if let Err(error) = cleanup {
                eprintln!("hosted cleanup after a panic failed: {error}");
            }
            std::panic::resume_unwind(panic)
        }
    }
}
