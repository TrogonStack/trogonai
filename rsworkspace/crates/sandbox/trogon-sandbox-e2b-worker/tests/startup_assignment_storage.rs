#![cfg(unix)]

mod authority_support;
mod worker_process_support;

use std::time::Duration;

use async_nats::jetstream::{self, kv, stream};
use authority_support::{Api, Boxed};
use futures::StreamExt;
use trogon_sandbox_contract::{ProviderKind, WorkerHeartbeat};
use trogon_sandbox_protocol::{DispatchSigner, subjects};
use worker_process_support::{Nats, Process};

const BUCKET: &str = "sandbox-e2b-assignments";

#[derive(Clone, Copy)]
enum Policy {
    Memory,
    Expiring,
    Evicting,
    MessageTtl,
}

/// Replaces the empty bucket with one whose stream carries the unsafe policy.
async fn plant_unsafe_bucket(context: &jetstream::Context, policy: Policy) -> Result<(), Boxed> {
    let store = context
        .create_key_value(kv::Config {
            bucket: BUCKET.to_owned(),
            ..Default::default()
        })
        .await?;
    let mut native_stream = store.stream.clone();
    let mut config = native_stream.info().await?.config.clone();
    context.delete_stream(config.name.as_str()).await?;
    match policy {
        Policy::Memory => config.storage = stream::StorageType::Memory,
        Policy::Expiring => config.max_age = Duration::from_secs(3600),
        Policy::Evicting => {
            config.discard = stream::DiscardPolicy::Old;
            config.discard_new_per_subject = false;
        }
        Policy::MessageTtl => {
            config.allow_message_ttl = true;
            config.metadata.insert("_nats.req.level".into(), "1".into());
        }
    }
    context.create_stream(config).await?;
    Ok(())
}

fn start_worker(nats: &Nats, api: &Api, ledger: &std::path::Path, signer: &DispatchSigner) -> Result<Process, Boxed> {
    worker_process_support::worker(&nats.url, &api.url, &api.url, ledger, signer)
}

/// Waits until the worker process has exited, and returns whether it published a heartbeat first.
async fn refused(worker: &mut Process, heartbeats: &mut async_nats::Subscriber) -> Result<bool, Boxed> {
    tokio::time::timeout(Duration::from_secs(20), async {
        while worker.is_alive()? {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), Boxed>(())
    })
    .await??;
    Ok(tokio::time::timeout(Duration::from_millis(200), heartbeats.next())
        .await
        .is_err())
}

async fn unsafe_storage_refuses_startup(policy: Policy) -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = jetstream::new(nats.client.clone());
    plant_unsafe_bucket(&context, policy).await?;
    let mut api = Api::start().await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("startup-storage-fixture", [79; 32]);
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    nats.client.flush().await?;
    let mut worker = start_worker(&nats, &api, &ledger, &signer)?;
    assert!(
        refused(&mut worker, &mut heartbeats).await?,
        "unsafe assignment storage must refuse readiness before any heartbeat"
    );
    worker.stop()?;
    api.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn memory_assignment_storage_refuses_startup() -> Result<(), Boxed> {
    unsafe_storage_refuses_startup(Policy::Memory).await
}

#[tokio::test]
async fn expiring_assignment_storage_refuses_startup() -> Result<(), Boxed> {
    unsafe_storage_refuses_startup(Policy::Expiring).await
}

#[tokio::test]
async fn evicting_assignment_storage_refuses_startup() -> Result<(), Boxed> {
    unsafe_storage_refuses_startup(Policy::Evicting).await
}

#[tokio::test]
async fn message_ttl_assignment_storage_refuses_startup() -> Result<(), Boxed> {
    unsafe_storage_refuses_startup(Policy::MessageTtl).await
}

#[tokio::test]
async fn safe_assignment_storage_is_created_and_accepted_again_after_a_restart() -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = jetstream::new(nats.client.clone());
    let mut api = Api::start().await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("startup-storage-fixture", [79; 32]);
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    nats.client.flush().await?;
    for _ in 0..2 {
        let mut worker = start_worker(&nats, &api, &ledger, &signer)?;
        let heartbeat = tokio::time::timeout(Duration::from_secs(30), heartbeats.next())
            .await?
            .ok_or("the worker did not become ready on safe storage")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
        assert_eq!(heartbeat.capacity.current_leases, 0);
        assert!(worker.is_alive()?);
        worker.kill()?;
    }
    let store = context.get_key_value(BUCKET).await?;
    let mut native_stream = store.stream.clone();
    assert_eq!(
        native_stream.info().await?.state.messages,
        0,
        "the bucket stays empty without any lease"
    );
    api.shutdown().await;
    Ok(())
}
