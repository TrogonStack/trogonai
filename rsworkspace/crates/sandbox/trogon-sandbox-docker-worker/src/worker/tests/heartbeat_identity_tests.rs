//! A Docker grant names the same worker its heartbeat publishes.

use std::time::Duration;

use anyhow::Context;
use bollard::query_parameters::RemoveContainerOptionsBuilder;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ProviderKind, WorkerHeartbeat, WorkerId};
use trogon_sandbox_protocol::subjects::worker_heartbeat;
use trogon_sandbox_providers::{DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER};

use super::docker_lease::{docker_provider, node_lease_request};
use super::*;
use crate::fresh_id::fresh_id;
use crate::heartbeat::spawn_heartbeat_task;

const BOUND: Duration = Duration::from_secs(5);

fn owned_by(inspected: &bollard::models::ContainerInspectResponse, owner: &str, lease_id: &LeaseId) -> bool {
    inspected
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref())
        .is_some_and(|labels| {
            labels.get(DOCKER_LABEL_OWNER).map(String::as_str) == Some(owner)
                && labels.get(DOCKER_LABEL_LEASE_ID).map(String::as_str) == Some(lease_id.as_str())
        })
}

async fn request_lease(client: &Client, dispatch: &ProviderDispatchPayload) -> anyhow::Result<ProviderLease> {
    loop {
        match client
            .request(LEASE_SUBJECT, serde_json::to_vec(dispatch)?.into())
            .await
        {
            Ok(reply) => return Ok(serde_json::from_slice::<ProviderLease>(&reply.payload)?),
            Err(error) if error.kind() == async_nats::client::RequestErrorKind::NoResponders => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Cancels the lease through the product, then removes whatever the product left behind so the daemon ends clean,
/// and fails if the product needed that help.
async fn clean_owned(
    docker: &DockerWorkerProvider,
    native: &bollard::Docker,
    owner: &str,
    lease_id: &LeaseId,
    allocated: Option<SandboxId>,
) -> anyhow::Result<()> {
    let mut owned = docker.inner.engine().list_by_label(DOCKER_LABEL_OWNER, owner).await?;
    owned.extend(allocated.map(|id| id.as_str().to_owned()));
    let product = tokio::time::timeout(Duration::from_secs(20), docker.cancel(lease_id)).await;
    let remaining = docker.inner.engine().list_by_label(DOCKER_LABEL_OWNER, owner).await?;
    let reconciled = !remaining.is_empty();
    for id in remaining {
        let inspected = native.inspect_container(&id, None).await?;
        anyhow::ensure!(
            inspected.id.as_deref() == Some(id.as_str()) && owned_by(&inspected, owner, lease_id),
            "identity fixture reconciliation refused a changed owner"
        );
        native
            .remove_container(
                &id,
                Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
            )
            .await?;
        owned.push(id);
    }
    owned.sort();
    owned.dedup();
    let mut absent = true;
    for id in &owned {
        absent &= matches!(
            native.inspect_container(id, None).await,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
        );
    }
    let empty = docker
        .inner
        .engine()
        .list_by_label(DOCKER_LABEL_OWNER, owner)
        .await?
        .is_empty();
    anyhow::ensure!(
        absent && empty,
        "identity fixture exact native cleanup remains unconfirmed"
    );
    product.context("identity fixture product cleanup exceeded its bound")??;
    anyhow::ensure!(
        !reconciled,
        "fixture reconciliation cannot replace product cleanup acceptance"
    );
    Ok(())
}

#[tokio::test]
async fn authority_grant_uses_the_same_worker_identity_as_its_published_heartbeat() -> anyhow::Result<()> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let owner = format!("docker-authority-{}", fresh_id());
    let worker_id = WorkerId::new(owner.clone())?;
    let docker = Arc::new(docker_provider(
        &client,
        format!("nats://{}", server.address()),
        &owner,
    )?);
    let provider: Arc<dyn Provider> = docker.clone();
    let native = bollard::Docker::connect_with_defaults()?;
    let lease_id = LeaseId::new(fresh_id());
    let request = node_lease_request(lease_id.as_str())?;
    let mut heartbeats = client
        .subscribe(worker_heartbeat(ProviderKind::LocalDocker, &worker_id))
        .await?;
    client.flush().await?;
    let cancel = CancellationToken::new();
    let current = Arc::new(AtomicU32::new(0));
    spawn_heartbeat_task(
        client.clone(),
        worker_id.clone(),
        Default::default(),
        4,
        current.clone(),
        docker.clone(),
        cancel.clone(),
    );
    let worker = tokio::spawn(run(
        client.clone(),
        jetstream::new(client.clone()),
        provider,
        docker.clone(),
        owner.clone(),
        cancel.clone(),
        current,
    ));
    let mut allocated = None;
    let observation = async {
        let heartbeat = tokio::time::timeout(BOUND, heartbeats.next())
            .await
            .context("authority identity heartbeat exceeded its bound")?
            .context("heartbeat feed closed")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
        let dispatch = ProviderDispatchPayload {
            lease_id: lease_id.clone(),
            request,
            correlation_id: None,
            resolved_drives: Vec::new(),
            target: None,
        };
        let grant = tokio::time::timeout(BOUND, request_lease(&client, &dispatch))
            .await
            .context("authority identity dispatch exceeded its bound")??;
        allocated = Some(grant.sandbox_id.clone());
        let id = grant.sandbox_id.as_str();
        anyhow::ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "the identity fixture must receive an actual canonical Docker grant"
        );
        let inspected = tokio::time::timeout(BOUND, native.inspect_container(id, None)).await??;
        anyhow::ensure!(
            inspected.id.as_deref() == Some(id)
                && inspected.state.as_ref().and_then(|state| state.running) == Some(true)
                && owned_by(&inspected, &owner, &lease_id),
            "the identity fixture did not hold its exact running owned native lease"
        );
        anyhow::Ok((heartbeat, grant))
    }
    .await;
    let cleanup = tokio::time::timeout(
        Duration::from_secs(30),
        clean_owned(&docker, &native, &owner, &lease_id, allocated),
    )
    .await;
    cancel.cancel();
    let stopped = tokio::time::timeout(BOUND, worker).await;
    cleanup.context("identity fixture cleanup exceeded its bound")??;
    stopped.context("worker shutdown exceeded its bound")???;
    let (heartbeat, grant) = observation?;
    assert_eq!(
        grant.worker_id,
        Some(heartbeat.worker_id),
        "a fresh Docker grant must be attributed to its actual heartbeat and authority worker"
    );
    Ok(())
}
