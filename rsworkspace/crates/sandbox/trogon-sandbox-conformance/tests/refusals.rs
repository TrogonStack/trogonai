//! Runs the suite against workers that misbehave, and expects it to say so.

mod support;

use std::error::Error;

use support::{Cleanup, Config, Echo, Exec, heartbeat, keys, shell_target};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{assert_conforms, run_suite};
use trogon_sandbox_contract::{CapabilitySupport, ProviderKind, WorkerDispatchProtocol, WorkerId};

const KIND: ProviderKind = ProviderKind::Subprocess;

#[tokio::test(flavor = "multi_thread")]
async fn an_unsigned_suite_does_not_get_past_a_worker_that_requires_signatures() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let (_, verifier) = keys()?;
    let mut config = Config::new(KIND, Exec::Shell);
    config.verifier = Some(verifier);
    let _worker = support::spawn(client.clone(), config).await?;
    let target = shell_target(client, KIND)?;
    let refused = assert_conforms(&target).await.err().ok_or("the worker conformed")?;
    assert!(refused.0.len() > 20);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_without_evidence_fails_the_suite() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut config = Config::new(KIND, Exec::Shell);
    config.evidence = String::new();
    let _worker = support::spawn(client.clone(), config).await?;
    let target = shell_target(client, KIND)?;
    assert!(!run_suite(&target).await.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_for_another_assignment_fails_the_suite_and_is_cleaned_up() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let (signer, _) = keys()?;
    let worker = WorkerId::new("worker-1")?;
    let protocol = WorkerDispatchProtocol::TargetedV1;
    for (echo, cleanup) in [
        (Echo::Worker(WorkerId::new("worker-2")?), Cleanup::Confirm),
        (Echo::Attempt, Cleanup::Fail),
        (Echo::Silent, Cleanup::Confirm),
    ] {
        let mut config = Config::new(KIND, Exec::Shell);
        (config.echo, config.cleanup) = (echo, cleanup);
        let fake = support::spawn(client.clone(), config).await?;
        let selected = heartbeat(KIND, &worker, protocol, CapabilitySupport::Supported);
        let mut target = shell_target(client.clone(), KIND)?.for_worker(&selected)?;
        target.signer = Some(signer.clone());
        target.timeout = std::time::Duration::from_secs(5);
        assert!(!run_suite(&target).await.is_empty());
        drop(fake);
    }
    Ok(())
}
