//! Runs the suite against workers that are selected from a heartbeat, signed or not.

mod support;

use std::error::Error;

use support::{Config, Exec, heartbeat, keys, shell_target};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{Target, assert_conforms};
use trogon_sandbox_contract::{CapabilitySupport, ProviderKind, WorkerDispatchProtocol, WorkerId};

const KIND: ProviderKind = ProviderKind::Subprocess;

async fn targeted(epoch: CapabilitySupport) -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let (signer, verifier) = keys()?;
    let mut config = Config::new(KIND, Exec::Shell);
    config.verifier = Some(verifier);
    let _worker = support::spawn(client.clone(), config).await?;
    let worker = WorkerId::new("worker-1")?;
    let mut target =
        shell_target(client, KIND)?.for_worker(&heartbeat(KIND, &worker, WorkerDispatchProtocol::TargetedV1, epoch))?;
    target.signer = Some(signer);
    assert_conforms(&target).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_targeted_worker_that_supports_epochs_conforms() -> Result<(), Box<dyn Error>> {
    targeted(CapabilitySupport::Supported).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_targeted_worker_without_epochs_conforms() -> Result<(), Box<dyn Error>> {
    targeted(CapabilitySupport::Unsupported).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_worker_that_names_itself_conforms() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = WorkerId::new("worker-1")?;
    let mut config = Config::new(KIND, Exec::Shell);
    config.worker_id = Some(worker.clone());
    let _worker = support::spawn(client.clone(), config).await?;
    let selected = heartbeat(
        KIND,
        &worker,
        WorkerDispatchProtocol::Legacy,
        CapabilitySupport::Unsupported,
    );
    let target = shell_target(client, KIND)?.for_worker(&selected)?;
    assert_conforms(&target).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_of_another_provider_cannot_be_selected() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = WorkerId::new("worker-1")?;
    let protocol = WorkerDispatchProtocol::TargetedV1;
    let other = heartbeat(ProviderKind::Mock, &worker, protocol, CapabilitySupport::Supported);
    assert!(Target::new(client.clone(), KIND).for_worker(&other).is_err());
    let mut mixed = heartbeat(KIND, &worker, protocol, CapabilitySupport::Supported);
    mixed.profile.provider = ProviderKind::Mock;
    assert!(Target::new(client, KIND).for_worker(&mixed).is_err());
    Ok(())
}
