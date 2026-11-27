//! Runs the suite against a worker that refuses unsigned dispatches.
//!
//! The worker's verifier is process-wide and read from the environment on first use, so this file holds a single
//! test that sets the environment before anything starts and then checks both directions on one worker: the suite
//! and the broker's entity lifecycle pass when signed, and the suite is refused when it is not.

mod support;

use std::error::Error;

use support::entity::run_entity_checks;
use support::memory::memory_target;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_broker::entity_store::SandboxEntityMode;
use trogon_sandbox_conformance::{CheckFailureError, SuiteFailureError, assert_conforms, run_suite, signer_from_env};
use trogon_sandbox_memory_worker::{MemoryWorker, MemoryWorkerConfig};
use trogon_sandbox_protocol::{DispatchSigner, ENV_PUBLIC_KEYS, ENV_SIGNING_KEY, ENV_VERIFY_MODE};
use trogon_std::env::SystemEnv;

/// The message a verifying worker answers an unsigned dispatch with.
const UNSIGNED_REFUSAL: &str = "dispatch is not signed";

/// Whether a check failed because the worker refused its unsigned dispatch. The refusal reaches the suite as the
/// worker's reply body, so the wire message is what identifies it.
fn refused_as_unsigned(failure: &SuiteFailureError) -> bool {
    match &failure.failure {
        CheckFailureError::Mismatch { observed, .. } => observed.contains(UNSIGNED_REFUSAL),
        CheckFailureError::Decode { reply, .. } => reply.contains(UNSIGNED_REFUSAL),
        _ => false,
    }
}

#[test]
fn a_require_mode_worker_conforms_only_to_a_signing_suite() -> Result<(), Box<dyn Error>> {
    let (seed, public) = DispatchSigner::generate_keypair_b64();
    // SAFETY: this binary holds one test and nothing else runs yet; the runtime is built afterwards.
    unsafe {
        std::env::set_var(ENV_VERIFY_MODE, "require");
        std::env::set_var(ENV_PUBLIC_KEYS, format!("k1:{public}"));
        std::env::set_var(ENV_SIGNING_KEY, seed);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(require_mode())
}

async fn require_mode() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = MemoryWorker::start(client.clone(), MemoryWorkerConfig::default()).await?;

    // The broker signs dispatches and cancels with the same key, so the entity lifecycle also runs against the
    // require-mode worker.
    for mode in [SandboxEntityMode::Shadow, SandboxEntityMode::On] {
        let failures = run_entity_checks(&worker, client.clone(), mode, signer_from_env(&SystemEnv)?).await?;
        assert!(failures.is_empty(), "{mode:?}: {failures:?}");
    }

    let unsigned = memory_target(client.clone());
    let failures = run_suite(&unsigned).await;
    assert!(
        failures.iter().any(refused_as_unsigned),
        "an unsigned suite is refused: {failures:?}"
    );

    let mut signed = memory_target(client);
    signed.signer = signer_from_env(&SystemEnv)?;
    assert!(signed.signer.is_some(), "the key variables are set");
    assert_conforms(&signed).await?;
    worker.shutdown().await;
    Ok(())
}
