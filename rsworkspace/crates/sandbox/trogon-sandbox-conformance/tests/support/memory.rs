//! Targets for the in-process memory worker and for workers that are already running elsewhere.

use std::time::Duration;

use trogon_sandbox_conformance::Target;
use trogon_sandbox_contract::ProviderKind;
use trogon_std::env::{ReadEnv, SystemEnv};

use super::Fallible;

/// Names the NATS server an already running worker is attached to.
pub const NATS_URL: &str = "SANDBOX_CONFORMANCE_NATS_URL";

/// Overrides the image leased from image-based workers.
pub const IMAGE: &str = "SANDBOX_CONFORMANCE_IMAGE";

/// A target that declares everything the memory worker supports.
pub fn memory_target(client: async_nats::Client) -> Target {
    let mut target = Target::new(client, ProviderKind::Mock);
    target.timeout = Duration::from_secs(10);
    target.strict_unknown_command_exit = true;
    target.supports_pause_resume = true;
    target.checkpoint_running_unsupported = true;
    target.graceful_stop = true;
    target.command_restart = true;
    target.named_processes = true;
    target.process_readiness = true;
    target.process_command = vec!["serve".to_owned(), "7777".to_owned()];
    target.process_needle = "listening on 7777".to_owned();
    target.process_listener = Some((vec!["serve".to_owned(), "7788".to_owned()], 7788));
    target.native_passthrough = true;
    target.redispatch_idempotent = true;
    target
}

/// A target for a worker that is already running against the NATS server named by [`NATS_URL`]. It signs with the
/// broker's key variables when they are set, so a worker in `require` mode accepts it.
pub async fn external_target(kind: ProviderKind) -> Fallible<Target> {
    let url = SystemEnv
        .var(NATS_URL)
        .map_err(|_| format!("set {NATS_URL} to the NATS server the worker is attached to"))?;
    let client = async_nats::connect(url).await?;
    let mut target = Target::new(client, kind).signed_from_env(&SystemEnv)?;
    if let Ok(image) = SystemEnv.var(IMAGE) {
        target.image = image;
    }
    Ok(target)
}

/// Waits for the worker's last release to land: releases are published without waiting for the worker.
pub async fn drained(worker: &trogon_sandbox_memory_worker::MemoryWorker) -> usize {
    for _ in 0..50 {
        if worker.lease_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    worker.lease_count()
}
