// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! Signed command requests and a configured executor for the native launch tests.

use std::time::SystemTime;

use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, LeaseId, TenantId, WorkerId};
use trogon_sandbox_e2b_worker::E2bExec;
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, VerbScope, VerifiedVerbScope};

use crate::native_command_support::Boxed;
use crate::native_command_support::command_deadline_support::{TestResult, exec};

/// The tenant every request of these tests is signed for.
pub const TENANT: &str = "native-launch-tenant";

/// A request signed for `subject` and verified, with the exact payload bytes the signature covers.
pub fn verified(
    request: &impl serde::Serialize,
    subject: &str,
    lease: &str,
) -> TestResult<(VerifiedVerbScope, Vec<u8>)> {
    let signer = DispatchSigner::new("native-launch-fixture", [7; 32]);
    let verifier =
        DispatchVerifier::new(EnforcementMode::Require).with_key(signer.key_id(), &signer.public_key_b64())?;
    let scope = VerbScope {
        tenant: TenantId::new(TENANT)?,
        lease_id: LeaseId::new(lease),
        worker_id: Some(WorkerId::new("native-launch-worker")?),
        attempt_id: Some(DispatchAttemptId::new("native-launch-attempt")?),
        epoch: Some(LeaseEpoch::new(1)?),
    };
    let payload = serde_json::to_vec_pretty(request)?;
    let header = signer.sign_verb(subject, &payload, &scope)?;
    let verified = verifier.verify_verb_scope(subject, &header, &payload, false, SystemTime::now())?;
    Ok((verified, payload))
}

/// An executor that records launch custody in the JetStream context.
pub fn configured(context: async_nats::jetstream::Context) -> TestResult<E2bExec> {
    let exec = exec()?;
    exec.configure_command_launches(context)?;
    Ok(exec)
}

/// Converts an error of the thread-safe guest support into the error type of the HTTP fixture.
pub fn plain<T>(result: Result<T, Boxed>) -> TestResult<T> {
    Ok(result.map_err(|error| error.to_string())?)
}
