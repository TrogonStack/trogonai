//! Authority reclaim must never resume a container the user paused.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use async_trait::async_trait;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    OwnedLease, SandboxFreezeResponse, WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId,
};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityGuard, AuthorityMode, AuthorityOracle, OracleError};

use super::docker_lease::{docker_provider, node_lease_request};
use super::*;
use crate::fresh_id::fresh_id;

/// Answers that the worker owns every lease it asks about once `assigned` is set, and none before.
struct DockerAuthorityOracle {
    assigned: AtomicBool,
}

#[async_trait]
impl AuthorityOracle for DockerAuthorityOracle {
    async fn owned(&self, request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        let owned = if self.assigned.load(Ordering::SeqCst) {
            request
                .lease_ids
                .iter()
                .map(|lease_id| OwnedLease {
                    lease_id: lease_id.clone(),
                    epoch: None,
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(WorkerAuthorityResponse {
            owned,
            not_assigned: Vec::new(),
        })
    }
}

async fn container_paused(docker: &bollard::Docker, container_id: &str) -> anyhow::Result<bool> {
    Ok(docker
        .inspect_container(container_id, None)
        .await?
        .state
        .and_then(|state| state.paused)
        .unwrap_or(false))
}

async fn docker_manual_pause_is_preserved(preexisting_pause: bool) -> anyhow::Result<()> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let owner = format!("authority-pause-{}", fresh_id());
    let provider = Arc::new(docker_provider(
        &client,
        format!("nats://{}", server.address()),
        &owner,
    )?);
    let lease_id = LeaseId::new(fresh_id());
    let request = node_lease_request(lease_id.as_str())?;
    let granted = match provider.lease(&request, &lease_id).await {
        Ok(granted) => granted,
        Err(error) => {
            provider.inner.sweep_owned().await?;
            return Err(error.into());
        }
    };
    let observation: anyhow::Result<(bool, bool)> = async {
        let active: ActiveLeases = Arc::new(Mutex::new(HashMap::from([(
            lease_id.clone(),
            CancellationToken::new(),
        )])));
        let oracle = Arc::new(DockerAuthorityOracle {
            assigned: AtomicBool::new(false),
        });
        let host = Arc::new(DockerAuthorityHost {
            docker_provider: provider.clone(),
            active,
            grants: Arc::new(RedispatchGuard::new()),
            terminals: Arc::new(DockerTerminals::with_replay_bytes(0)),
            client: client.clone(),
        });
        let mut config = AuthorityConfig::new(AuthorityMode::Enforce);
        config.min_lease_age = Duration::ZERO;
        config.confirmations = 1;
        let mut guard = AuthorityGuard::new(config, WorkerId::new(owner.clone())?, oracle.clone(), host);
        if preexisting_pause {
            anyhow::ensure!(provider.set_paused_for_lease(&lease_id, true).await == SandboxFreezeResponse::Done);
        }
        guard.tick().await;
        let docker = bollard::Docker::connect_with_defaults()?;
        let before = container_paused(&docker, granted.sandbox_id.as_str()).await?;
        if !preexisting_pause {
            anyhow::ensure!(provider.set_paused_for_lease(&lease_id, true).await == SandboxFreezeResponse::Done);
        }
        oracle.assigned.store(true, Ordering::SeqCst);
        guard.tick().await;
        let after = container_paused(&docker, granted.sandbox_id.as_str()).await?;
        Ok((before, after))
    }
    .await;
    provider.cancel(&lease_id).await?;
    let (before, after) = observation?;
    assert!(
        before,
        "the owned Docker container must actually be paused before authority returns"
    );
    assert!(after, "authority recovery must preserve an explicit user pause");
    Ok(())
}

#[tokio::test]
async fn authority_reclaim_does_not_resume_a_container_already_paused_by_the_user() -> anyhow::Result<()> {
    docker_manual_pause_is_preserved(true).await
}

#[tokio::test]
async fn authority_reclaim_preserves_a_user_pause_made_after_the_guard_froze_the_container() -> anyhow::Result<()> {
    docker_manual_pause_is_preserved(false).await
}
