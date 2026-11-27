// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! Signed, verified command requests for handlers that take the verified scope of a request.

use std::time::SystemTime;

use serde::Serialize;

use crate::native_command_support::command_deadline_support;
use trogon_sandbox_contract::{
    DispatchAttemptId, ExecutionId, KillCommandRequest, LeaseEpoch, LeaseId, RunCommandRequest, RunCommandResponse,
    TenantId, WorkerId,
};
use trogon_sandbox_e2b_worker::{E2bExec, LeaseSandbox};
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, VerbScope, VerifiedVerbScope};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult<T = ()> = Result<T, Boxed>;

fn plain<T>(result: Result<T, Box<dyn std::error::Error>>) -> TestResult<T> {
    result.map_err(|error| error.to_string().into())
}

/// A sandbox whose envd endpoint is the fixture at `url`.
pub fn lease_sandbox(url: &str, id: &str) -> TestResult<LeaseSandbox> {
    plain(command_deadline_support::lease_sandbox(url, id))
}

pub fn exec() -> TestResult<E2bExec> {
    plain(command_deadline_support::exec())
}

pub fn run_request(value: serde_json::Value) -> TestResult<RunCommandRequest> {
    plain(command_deadline_support::run_request(value))
}

/// The execution id of a response that started a detached command.
pub fn started(response: &RunCommandResponse) -> TestResult<ExecutionId> {
    plain(command_deadline_support::started(response))
}

/// A signer and the verifier that trusts it, enforcing signatures.
pub struct Signing {
    signer: DispatchSigner,
    verifier: DispatchVerifier,
}

impl Signing {
    pub fn new(key_id: &str, seed: u8) -> TestResult<Self> {
        let signer = DispatchSigner::new(key_id, [seed; 32]);
        let verifier =
            DispatchVerifier::new(EnforcementMode::Require).with_key(signer.key_id(), &signer.public_key_b64())?;
        Ok(Self { signer, verifier })
    }

    /// Signs `request` for `subject` and `scope`, then verifies it as the worker would. The payload is the body the
    /// verified scope is bound to.
    pub fn verify<T: Serialize>(
        &self,
        subject: &str,
        request: &T,
        scope: &VerbScope,
    ) -> TestResult<(VerifiedVerbScope, Vec<u8>)> {
        let payload = serde_json::to_vec_pretty(request)?;
        let header = self.signer.sign_verb(subject, &payload, scope)?;
        let verified = self
            .verifier
            .verify_verb_scope(subject, &header, &payload, false, SystemTime::now())?;
        Ok((verified, payload))
    }
}

/// The assignment a launch is signed for.
pub fn scope(tenant: &str, lease: &str, worker: &str, attempt: &str) -> TestResult<VerbScope> {
    Ok(VerbScope {
        tenant: TenantId::new(tenant)?,
        lease_id: LeaseId::new(lease),
        worker_id: Some(WorkerId::new(worker)?),
        attempt_id: Some(DispatchAttemptId::new(attempt)?),
        epoch: Some(LeaseEpoch::FIRST),
    })
}

pub fn kill_request(lease: &str, execution: ExecutionId) -> KillCommandRequest {
    command_deadline_support::kill_request(lease, execution)
}
